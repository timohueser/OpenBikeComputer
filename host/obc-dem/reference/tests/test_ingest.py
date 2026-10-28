"""The reference archive's contract, held over synthetic rasters.

Every test writes its own source raster, so nothing here needs a network or a credential.
The two facts the archive stands on are the exact lattice — integer microdegrees in, the
same integers back out of the GeoTIFF — and the pooling rule: a lattice pixel keeps the
maximum of the source pixels whose centres lie in it, which is what lets a 7 m archive
carry a one-pixel rock tower without spreading it over its neighbours.
"""

import hashlib
import json
import os
import shutil
import sys
import unittest
import unittest.mock
from pathlib import Path
from tempfile import TemporaryDirectory

import numpy as np
import rasterio
from rasterio.crs import CRS
from rasterio.transform import Affine

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import ingest  # noqa: E402
import merge  # noqa: E402

LV95 = CRS.from_epsg(2056)
# A 200 m square of 1 m posts near Engelberg, high enough that no rounding hides a defect.
EAST, NORTH, SIDE = 2670000.0, 1200000.0, 200
PLATEAU, TOWER = 1000.4, 1500.6
TOWER_ROW, TOWER_COL = 50, 50


def grid(step=1.0, east=EAST, north=NORTH, rotation=0.0):
    """A source transform: north-up by default, rotated when a test asks for it."""

    return Affine.translation(east, north) * Affine.rotation(rotation) * Affine.scale(step, -step)


def source_raster(path, values, dtype="float32", nodata=None, transform=None, scales=None):
    height, width = values.shape
    profile = {
        "driver": "GTiff", "height": height, "width": width, "count": 1, "dtype": dtype,
        "crs": LV95, "transform": transform if transform is not None else grid(),
    }
    if nodata is not None:
        profile["nodata"] = nodata
    with rasterio.open(path, "w", **profile) as dst:
        dst.write(values.astype(dtype), 1)
        if scales is not None:
            dst.scales, dst.offsets = scales
    return path


def plateau_with_tower(plateau=PLATEAU, tower=TOWER, row=TOWER_ROW, col=TOWER_COL):
    values = np.full((SIDE, SIDE), plateau, dtype="float32")
    values[row, col] = tower
    return values


def at(lon_u, lat_u):
    """The archive tile and the row and column in its north-up array of a microdegree point."""

    r, c = ingest.pixel_index(lat_u), ingest.pixel_index(lon_u)
    ti, tj = r // ingest.TILE_PX, c // ingest.TILE_PX
    return ti, tj, (ti + 1) * ingest.TILE_PX - 1 - r, c - tj * ingest.TILE_PX


def archive_pixel(transform, row, col):
    """`at` of one LV95 source pixel's centre."""

    x, y = ingest.source_xy(transform, np.array([row + 0.5]), np.array([col + 0.5]))
    lon, lat = rasterio.warp.transform(LV95, ingest.WGS84, list(x), list(y))
    return at(int(np.floor(lon[0] * ingest.DEGREE)), int(np.floor(lat[0] * ingest.DEGREE)))


def wgs84_grid(step_u, ti=4812, tj=4223, row=100, col=100):
    """A source already in EPSG:4326, on lattice lines, with a whole-microdegree step.

    With `step_u = 128` every pixel centre sits **exactly** on a lattice line, which is the
    case a projection cannot round-trip and a float floor can put on the wrong side.
    """

    lon0 = ingest.GRID_ORIGIN + tj * ingest.TILE + col * ingest.STEP
    lat_top = ingest.GRID_ORIGIN + (ti + 1) * ingest.TILE - row * ingest.STEP
    return Affine(step_u / 1e6, 0, lon0 / 1e6, 0, -step_u / 1e6, lat_top / 1e6), lon0, lat_top


def local_source(key, country="Testland"):
    return ingest.Source(key, country, f"{key} test product", 1.0, "CC0", "EGM2008")


def tile_problems(path, ti, tj, pool="max"):
    """Everything one tile of `pool` must be, including how the bytes are laid out.

    The layout is part of the contract because the reader streams tiles: a tile that is not
    deflated in 256 x 256 blocks, or is big-endian, reads correctly but not the way the
    baker was measured against.
    """

    dtype, nodata, predictor = ("int32", ingest.MEAN_NODATA, "2") if pool == ingest.MEAN else ("int16", ingest.NODATA, None)
    with rasterio.open(path) as src:
        found = {
            "size": (src.width, src.height), "dtype": src.dtypes[0], "nodata": src.nodata,
            "predictor": src.tags(ns="IMAGE_STRUCTURE").get("PREDICTOR"), "crs": src.crs, "bands": src.count,
            "area": src.tags().get("AREA_OR_POINT"), "blocks": src.block_shapes[0],
            "compression": src.compression.value.lower() if src.compression else "none",
        }
        expected = ingest.tile_window(ti, tj).transform
        moved = max(abs(a - b) for a, b in zip(src.transform[:6], expected[:6]))
    wanted = {"size": (ingest.TILE_PX, ingest.TILE_PX), "dtype": dtype, "nodata": nodata, "predictor": predictor,
              "crs": ingest.WGS84, "bands": 1, "area": "Area", "blocks": (256, 256), "compression": "deflate"}
    problems = [f"{name}: {found[name]}, not {value}" for name, value in wanted.items() if found[name] != value]
    if moved > 1e-9:
        problems.append(f"transform is {moved} degrees off the lattice")
    with path.open("rb") as handle:
        if handle.read(2) != b"II":
            problems.append("the TIFF header is not little-endian")
    return problems


class ArchiveCase(unittest.TestCase):
    def setUp(self):
        self.work = TemporaryDirectory()
        self.root = Path(self.work.name)
        self.inputs = self.root / "input"
        self.inputs.mkdir()
        self.addCleanup(self.work.cleanup)

    def stand_in(self, *keys):
        """Put a local source under a registry key, so a test needs no service."""

        for key in keys:
            real = ingest.SOURCES[key]
            ingest.SOURCES[key] = local_source(key)
            self.addCleanup(lambda k=key, s=real: ingest.SOURCES.__setitem__(k, s))


class Lattice(unittest.TestCase):
    def test_the_transform_is_the_integer_lattice(self):
        """The transform is built from microdegrees, and the file gives them back exactly."""

        with TemporaryDirectory() as work:
            ti, tj = 4809, 4324
            path = Path(work) / "tile.tif"
            ingest.write_tile(path, ti, tj, np.full((ingest.TILE_PX, ingest.TILE_PX), 7, dtype="int16"))
            with rasterio.open(path) as src:
                transform = src.transform
            lon0 = round(transform.c * ingest.DEGREE)
            lat_top = round(transform.f * ingest.DEGREE)
            self.assertEqual(lon0, ingest.GRID_ORIGIN + tj * ingest.TILE)
            self.assertEqual(lat_top, ingest.GRID_ORIGIN + (ti + 1) * ingest.TILE)
            self.assertEqual(round(transform.a * ingest.DEGREE), ingest.STEP)
            self.assertEqual(round(-transform.e * ingest.DEGREE), ingest.STEP)
            self.assertEqual(tile_problems(path, ti, tj), [])

    def test_a_box_over_the_antimeridian_or_the_pole_is_refused(self):
        for bbox in ((179.9, 46.0, -179.9, 46.1), (8.0, 46.0, 8.1, 90.5)):
            with self.subTest(bbox=bbox), self.assertRaises(ingest.Refuse):
                ingest.check_world(bbox, "--tile")

    def test_the_digest_is_the_pixel_array_and_not_the_file(self):
        """Two GDAL builds deflate the same heights into different bytes."""

        with TemporaryDirectory() as work:
            path = Path(work) / "tile.tif"
            pixels = np.arange(ingest.TILE_PX ** 2, dtype="int64").reshape(ingest.TILE_PX, -1).astype("int16")
            ingest.write_tile(path, 4809, 4324, pixels)
            digest = ingest.tile_digest(path)
            self.assertEqual(digest, hashlib.sha256(pixels.astype("<i2").tobytes()).hexdigest())
            with rasterio.open(path) as src:
                profile = src.profile
            with rasterio.open(path, "w", **{**profile, "compress": None, "tiled": False}) as dst:
                dst.write(pixels, 1)
            self.assertEqual(ingest.tile_digest(path), digest)


class Pool(ArchiveCase):
    """`pool_tile`: source rasters onto one tile of the lattice."""

    def pool(self, values, transform=None, **kw):
        """Pool one LV95 raster, and return the maximum of the tile that holds its tower."""

        transform = transform if transform is not None else grid()
        raster = source_raster(self.root / "source.tif", values, transform=transform, **kw)
        ti, tj, _, _ = archive_pixel(transform, TOWER_ROW, TOWER_COL)
        maximum, _ = ingest.pool_tile([raster], ingest.SOURCES["ch"], ti, tj)
        return maximum

    def test_the_tower_stays_one_pixel_wherever_it_stands(self):
        """The centre rule puts a one-pixel tower in exactly one archive pixel, always.

        Area-overlap resampling spreads it over two to four, because the archive pixel is
        about 7 m by 5 m and the tower's 1 m square touches more than one of them.
        """

        for row, col in ((50, 50), (51, 53), (7, 191), (123, 44), (0, 0), (199, 199)):
            with self.subTest(row=row, col=col):
                data = self.pool(plateau_with_tower(row=row, col=col))
                _, _, r, c = archive_pixel(grid(), row, col)
                self.assertEqual(int((data == round(TOWER)).sum()), 1)
                self.assertEqual(int(data[r, c]), round(TOWER))

    def test_a_degree_source_on_lattice_lines_is_placed_exactly(self):
        """Every centre of a 128 µdeg EPSG:4326 source is on a lattice line, and lands right.

        Consecutive centres are two lattice pixels apart, so each source pixel gets an
        archive pixel of its own: one pixel short of 100 x 100 would mean two centres
        collapsed onto one lattice line.
        """

        transform, lon0, lat_top = wgs84_grid(128)
        values = np.full((100, 100), 1000.0, dtype="float32")
        values[50, 50] = TOWER
        raster = source_raster(self.root / "wgs84.tif", values, transform=transform)
        with rasterio.open(raster, "r+") as dst:
            dst.crs = ingest.WGS84
        # The centre of source pixel (50, 50), in microdegrees, is a lattice line itself.
        lon_u = lon0 + 128 * 50 + 64
        lat_u = lat_top - 128 * 50 - 64
        self.assertEqual(lon_u % ingest.STEP, 0)
        ti, tj, r, c = at(lon_u, lat_u)
        data, _ = ingest.pool_tile([raster], ingest.SOURCES["ch"], ti, tj)

        self.assertEqual(int((data != ingest.NODATA).sum()), 100 * 100)
        self.assertEqual(int((data == round(TOWER)).sum()), 1)
        self.assertEqual(int(data[r, c]), round(TOWER))

    def test_a_rotated_source_loses_nothing(self):
        """A centre is a point, so a 30° rotated grid pools like any other."""

        transform = grid(rotation=30.0)
        data = self.pool(plateau_with_tower(), transform=transform)
        _, _, r, c = archive_pixel(transform, TOWER_ROW, TOWER_COL)
        self.assertEqual(int((data == round(TOWER)).sum()), 1)
        self.assertEqual(int(data[r, c]), round(TOWER))
        self.assertEqual(int((data != ingest.NODATA).sum()), int((data == round(PLATEAU)).sum()) + 1)

    def test_the_tile_is_whole_and_holds_nothing_the_source_did_not_say(self):
        data = self.pool(plateau_with_tower())
        self.assertEqual(data.shape, (ingest.TILE_PX, ingest.TILE_PX))
        # A 200 m square covers about 28 x 41 pixels of the tile; the rest is nodata.
        covered = int((data != ingest.NODATA).sum())
        self.assertTrue(1000 <= covered <= 1400, covered)
        self.assertEqual(covered, int((data == round(PLATEAU)).sum()) + 1)

    def test_voids_arrive_as_nodata_whatever_the_source_calls_them(self):
        """A float32 NaN, an integer sentinel and the float maximum are all one void."""

        for name, values, dtype, nodata in (
            ("nan", np.where(plateau_with_tower() > 1400, np.nan, PLATEAU), "float32", np.nan),
            ("sentinel", np.where(plateau_with_tower() > 1400, -9999, PLATEAU), "int16", -9999),
            ("huge", np.where(plateau_with_tower() > 1400, 3.4e38, PLATEAU), "float32", None),
            # −9999 with no nodata tag: only the plausible range catches this one.
            ("undeclared", np.where(plateau_with_tower() > 1400, -9999.0, PLATEAU), "float32", None),
        ):
            with self.subTest(void=name):
                data = self.pool(values, dtype=dtype, nodata=nodata)
                self.assertEqual(int(data.max()), round(PLATEAU))
                self.assertNotIn(round(TOWER), set(np.unique(data).tolist()))

    def test_a_rows_undeclared_void_is_absence(self):
        """GUGiK answers 0 for the land abroad and declares no nodata, so the row names it."""

        values = plateau_with_tower()
        values[:, :100] = 0.0
        raster = source_raster(self.root / "source.tif", values)
        ti, tj, _, _ = archive_pixel(grid(), TOWER_ROW, TOWER_COL)
        for key, zeros in (("ch", True), ("pl", False)):
            with self.subTest(key):
                maximum, _ = ingest.pool_tile([raster], ingest.SOURCES[key], ti, tj)
                self.assertEqual(bool((maximum == 0).any()), zeros)
                self.assertIn(round(PLATEAU), set(np.unique(maximum).tolist()))

    def test_a_scaled_band_is_refused_by_name(self):
        """A band with a scale or an offset is centimetres or worse, not metres."""

        raster = source_raster(self.inputs / "scaled.tif", plateau_with_tower(),
                               scales=((0.01,), (0.0,)))
        with self.assertRaises(ingest.Refuse) as caught:
            ingest.read_source(raster)
        self.assertIn("gdal_translate -unscale", str(caught.exception))

    def test_a_raster_that_cannot_be_opened_names_itself(self):
        broken = self.inputs / "half-written.tif"
        broken.write_bytes(b"II*\x00not a tiff")
        with self.assertRaises(ingest.Refuse) as caught:
            ingest.read_source(broken)
        self.assertIn("half-written.tif", str(caught.exception))
        self.assertIn(str(self.inputs), str(caught.exception))

    def test_the_void_fraction_is_measured(self):
        values = plateau_with_tower()
        values[:100] = np.nan  # the top half of the raster is void
        raster = source_raster(self.inputs / "half.tif", values, nodata=np.nan)
        _, _, _, _, voided = ingest.read_source(raster)
        self.assertAlmostEqual(voided, 0.5, places=3)

    def test_an_xyz_grid_is_placed_by_the_rows_grid(self):
        """Baden-Württemberg's DGM1 is XYZ text: no CRS, and GDAL reads no sidecar for it,
        so the row's grid places it. A missing point is a void, not a refusal."""

        lines = [f"{425000.5 + c:.2f} {5303019.5 - r:.2f} {1500.0 if (r, c) == (10, 10) else 1000.0:.2f}"
                 for r in range(20) for c in range(20) if (r, c) != (5, 5)]
        (self.inputs / "tile.xyz").write_text("\n".join(lines) + "\n")
        lon, lat = rasterio.warp.transform(CRS.from_epsg(25832), ingest.WGS84, [425000, 425020], [5303000, 5303020])
        source = ingest.SOURCES["de-bw"]
        rasters = ingest.local_rasters(source, self.inputs, (lon[0], lat[0], lon[1], lat[1]), self.root / "work")
        ti, tj, _, _ = at(int(lon[0] * ingest.DEGREE), int(lat[0] * ingest.DEGREE))
        data, _ = ingest.pool_tile(rasters, source, ti, tj)
        self.assertEqual(int(data.max()), 1500)
        self.assertEqual(int((data == 1500).sum()), 1)
        self.assertGreater(int((data == 1000).sum()), 0)

    def test_a_source_coarser_than_the_step_invents_nothing(self):
        """A 20 m source reaches the lattice pixels its centres land in, and no others."""

        values = np.full((10, 10), 1234.0, dtype="float32")
        raster = source_raster(self.root / "coarse.tif", values, transform=grid(step=20.0))
        ti, tj, _, _ = archive_pixel(grid(step=20.0), 5, 5)
        data, _ = ingest.pool_tile([raster], ingest.SOURCES["ch"], ti, tj)
        self.assertEqual(int((data == 1234).sum()), 100)  # one lattice pixel per source pixel centre
        self.assertEqual(int(((data != 1234) & (data != ingest.NODATA)).sum()), 0)


class BoxService(ingest.Source):
    """A service that answers a box with the columns of one EPSG:4326 raster inside it."""

    def __init__(self, raster):
        super().__init__("ch", "Testland", "box service", 1.0, "CC0", "EGM2008")
        self.raster, self.boxes = raster, []

    def fetch(self, bbox, workdir):
        self.boxes.append(bbox)
        west, _, east, _ = bbox
        with rasterio.open(self.raster) as src:
            values, t = src.read(1), src.transform
        centres = t.c * 1e6 + (np.arange(values.shape[1]) + 0.5) * t.a * 1e6
        cols = np.flatnonzero((centres >= west * 1e6) & (centres < east * 1e6))
        if cols.size == 0:
            return []
        workdir.mkdir(parents=True, exist_ok=True)
        path = source_raster(workdir / f"{len(self.boxes)}.tif", values[:, cols[0]:cols[-1] + 1],
                             transform=t * Affine.translation(int(cols[0]), 0))
        with rasterio.open(path, "r+") as dst:
            dst.crs = ingest.WGS84
        return [path]


class Fetch(ArchiveCase):
    """`ingest.py fetch`: one archive tile of a model, pooled, and nothing of the raw rasters."""

    def fetch(self, key, tile):
        """`ingest.py fetch` of `key` and `tile`: its exit code and the files it wrote."""

        out = self.root / "out"
        shutil.rmtree(out, ignore_errors=True)
        code = ingest.main(["fetch", key, "--tile", tile, "--work", str(self.root / "work"), "--out", str(out)])
        return code, sorted(str(path.relative_to(out)) for path in out.rglob("*") if path.is_file())

    def serve(self, service):
        real = ingest.SOURCES["ch"]
        ingest.SOURCES["ch"] = service
        self.addCleanup(ingest.SOURCES.__setitem__, "ch", real)
        return service

    def test_a_tile_holds_what_the_service_answered_for_it_and_the_raw_rasters_are_removed(self):
        # A 128 µdeg source that straddles two tiles, with a tower in each. Its columns 0 to 24
        # land in the west tile and 25 to 99 in the east one, one lattice pixel per centre.
        transform, _, _ = wgs84_grid(128, col=ingest.TILE_PX - 50)
        values = np.full((100, 100), 1000.0, dtype="float32")
        values[50, 10] = values[50, 60] = TOWER
        raster = source_raster(self.inputs / "wide.tif", values, transform=transform)
        with rasterio.open(raster, "r+") as dst:
            dst.crs = ingest.WGS84
        service = self.serve(BoxService(raster))
        digests = {}
        for (ti, tj), heights in (((4812, 4223), 25 * 100), ((4812, 4224), 75 * 100)):
            tile, name = f"{ti:04}/{tj:04}", f"{ti:04}-{tj:04}"
            path = self.root / "out" / "16" / f"{tile}.tif"
            files = [f"16/{tile}.tif", f"{name}.keys", f"{name}.mean.sha256", f"{name}.sha256", f"mean/16/{tile}.tif"]
            self.assertEqual(self.fetch("ch", name), (0, files))
            self.assertEqual(service.boxes[-1], ingest.tile_bounds(ti, tj))
            self.assertEqual(tile_problems(path, ti, tj), [])
            self.assertEqual(tile_problems(self.root / "out" / "mean" / "16" / f"{tile}.tif", ti, tj, "mean"), [])
            with rasterio.open(path) as src:
                data = src.read(1)
            self.assertEqual((int((data != ingest.NODATA).sum()), int((data == round(TOWER)).sum())), (heights, 1))
            digests[tile] = (self.root / "out" / f"{name}.sha256").read_text(encoding="utf-8")
            self.assertEqual(digests[tile], ingest.tile_digest(path))
            self.assertEqual((self.root / "out" / f"{name}.keys").read_text(encoding="utf-8"), "ch")
            self.assertFalse((self.root / "work").exists())
        tiles = dict(digests)

        index, listed, keys = self.root / "index.json", self.root / "digests.json", self.root / "keys.json"
        listed.write_text(json.dumps(digests), encoding="utf-8")
        keys.write_text(json.dumps({tile: ["ch"] for tile in tiles}), encoding="utf-8")
        self.assertEqual(ingest.main(["index-terrain", "--fetched", "2026-10-01", "--digests", str(listed),
                                      "--keys", str(keys), "--models", "ch", "--out", str(index)]), 0)
        index = json.loads(index.read_text(encoding="utf-8"))
        self.assertEqual((index["sha256"], index["sources"]["ch"]["fetched"]), (tiles, "2026-10-01"))
        self.assertEqual(index["contributors"], {tile: ["ch"] for tile in tiles})

        # A next version keeps the tiles before and the facts of a model that it did not fetch.
        first = sorted(tiles)[0]
        later = self.root / "later.json"
        listed.write_text(json.dumps({first: "new pixels"}), encoding="utf-8")
        keys.write_text(json.dumps({first: ["nl", "ch"]}), encoding="utf-8")
        self.stand_in("nl")
        self.assertEqual(ingest.main(["index-terrain", "--base", str(self.root / "index.json"), "--fetched", "2026-10-24",
                                      "--digests", str(listed), "--keys", str(keys), "--models", "nl",
                                      "--out", str(later)]), 0)
        later = json.loads(later.read_text(encoding="utf-8"))
        self.assertEqual(later["sha256"], {**tiles, first: "new pixels"})
        self.assertEqual((later["tiles"][first], later["contributors"][first]), ("nl", ["nl", "ch"]))
        self.assertEqual((later["sources"]["ch"]["fetched"], later["sources"]["nl"]["fetched"]), ("2026-10-01", "2026-10-24"))

    def test_the_mean_is_beside_the_maximum_over_every_raster_of_the_tile(self):
        """A 32 µdeg source puts four centres in each lattice pixel. Cut in two between the
        centres of one lattice pixel, its two rasters still give that pixel the mean of all
        four, in decimetres, beside their maximum in metres."""

        transform, _, _ = wgs84_grid(32)
        rows, cols = np.mgrid[0:40, 0:40]
        values = (1000.0 + 0.37 * cols + 0.11 * rows).astype("float32")
        values[20, 21] = TOWER
        rasters = []
        for name, first, last in (("west", 0, 21), ("east", 21, 40)):
            path = source_raster(self.inputs / f"{name}.tif", values[:, first:last],
                                 transform=transform * Affine.translation(first, 0))
            with rasterio.open(path, "r+") as dst:
                dst.crs = ingest.WGS84
            rasters.append(path)
        maximum, mean = ingest.pool_tile(rasters, ingest.SOURCES["ch"], 4812, 4223)

        quads = values.astype("float64").reshape(20, 2, 20, 2)
        held = np.s_[100:120, 100:120]
        self.assertTrue((mean[held] == np.floor(quads.mean(axis=(1, 3)) * 10 + 0.5)).all())
        self.assertTrue((maximum[held] == np.floor(quads.max(axis=(1, 3)) + 0.5)).all())
        self.assertEqual(int(maximum[110, 110]), round(TOWER))
        self.assertEqual((int((mean != ingest.MEAN_NODATA).sum()), int((maximum != ingest.NODATA).sum())),
                         (400, 400))

    def test_a_tile_without_a_height_is_a_none_file_and_a_failure_keeps_no_raw_raster(self):
        class Service(ingest.Source):
            boxes = []

            def fetch(self, bbox, workdir):
                self.boxes.append(bbox)
                workdir.mkdir(parents=True, exist_ok=True)
                void = np.full((SIDE, SIDE), -9999.0, dtype="float32")
                path = source_raster(workdir / "void.tif", void, nodata=-9999.0)
                if len(self.boxes) > 1:
                    raise ingest.Refuse("the service dropped the connection")
                return [path]

        self.serve(Service("ch", "Testland", "test", 1.0, "CC0", "EGM2008"))
        # The row `dtm-ch` has the extent [5.9, 45.8, 10.5, 47.9], and tile 4823-4256 straddles 10.5°.
        self.assertEqual(self.fetch("ch", "4823-4256"), (0, ["4823-4256.none"]))
        west, south, _, north = ingest.tile_bounds(4823, 4256)
        self.assertEqual(Service.boxes, [(west, south, 10.5, north)])
        self.assertEqual(self.fetch("ch", "4823-4300"), (0, ["4823-4300.none"]))
        self.assertEqual(len(Service.boxes), 1, "a tile outside the extent asks nothing")
        self.assertEqual(self.fetch("ch", "4823-4255"), (1, []))
        self.assertFalse((self.root / "work").exists())
        self.assertEqual(self.fetch("ch", "48230-4255")[0], 1)

    def test_a_delivery_is_fetched_from_the_directory_that_the_environment_names(self):
        """A row without a service reads its delivery like a credential, and leaves it as it is."""

        (self.inputs / "order").mkdir()
        raster = source_raster(self.inputs / "order" / "dem.tif", plateau_with_tower())
        ti, tj, _, _ = archive_pixel(grid(), TOWER_ROW, TOWER_COL)
        name = f"{ti:04}-{tj:04}"
        with unittest.mock.patch.dict(os.environ, {"OBC_REFERENCE_AU_INPUT": str(self.inputs)}):
            self.assertEqual(self.fetch("au", name)[0], 1, "the datum of the order is not confirmed")
            os.environ["OBC_REFERENCE_AU_DATUM"] = "AHD"
            path = f"16/{ti:04}/{tj:04}.tif"
            files = [path, f"{name}.keys", f"{name}.mean.sha256", f"{name}.sha256", f"mean/{path}"]
            self.assertEqual(self.fetch("au", name), (0, files))
            os.environ["OBC_REFERENCE_AU_INPUT"] = os.path.relpath(self.inputs)
            self.assertEqual(self.fetch("au", name)[0], 1, "a relative path names another directory in the step")
        self.assertEqual(sorted(path.name for path in self.inputs.rglob("*")), ["dem.tif", "order"])



class SwissSurveys(unittest.TestCase):
    def test_a_square_reads_its_newest_survey_only(self):
        """swissALTI3D publishes a square again after each survey: an older one would raise the
        maximum and move the mean."""

        def item(square, year, east):
            name = f"swissalti3d_{year}_{east}-1182_2_2056_5728.tif"
            return {"bbox": list(square), "properties": {"datetime": f"{year}-06-01T00:00:00Z"},
                    "assets": {name: {"href": f"https://example.invalid/{name}"},
                               name.replace("_2_", "_0.5_"): {"href": "https://example.invalid/fine.tif"}}}

        one, two = (8.38, 46.79, 8.39, 46.80), (8.39, 46.79, 8.40, 46.80)
        items = [item(one, 2019, 2672), item(two, 2019, 2673), item(one, 2022, 2672)]
        with unittest.mock.patch("ingest.sources.ch.stac_items", return_value=iter(items)):
            assets = ingest.SOURCES["ch"].assets((8.38, 46.79, 8.40, 46.80))
        self.assertEqual([name for name, _ in assets],
                         ["swissalti3d_2019_2673-1182_2_2056_5728.tif", "swissalti3d_2022_2672-1182_2_2056_5728.tif"])


class Fold(unittest.TestCase):
    """`fold_tiles`: a model into the tile of the version before."""

    def test_a_better_model_wins_where_it_has_a_height_and_a_worse_one_fills_the_gaps(self):
        nodata = ingest.NODATA
        existing = np.array([900, nodata, 900], dtype="int16")
        incoming = np.array([800, 800, nodata], dtype="int16")
        # `nl` ranks better than `es`, and `es` worse than `nl`.
        self.assertEqual(ingest.fold_tiles(existing, incoming, {"es"}, "nl", nodata).tolist(), [800, 800, 900])
        self.assertEqual(ingest.fold_tiles(existing, incoming, {"nl"}, "es", nodata).tolist(), [900, 800, 900])
        # A newer survey of the same model replaces its pixels, also where they are lower.
        self.assertEqual(ingest.fold_tiles(existing, incoming, {"es"}, "es", nodata).tolist(), [800, 800, 900])


class MergeStep(ArchiveCase):
    """The step of a map leaf: the tiles of a version of `terrain` that it reads, and their index."""

    def version(self, pool):
        """A version of `terrain` with two tiles of `pool`: its files by name, and the two tile ids."""

        prefix, dtype = ("mean/", "int32") if pool == "mean" else ("", "int16")
        files, tiles = {}, [(4812, 4223), (4812, 4224)]
        for ti, tj in tiles:
            name = f"{prefix}16/{ti:04}/{tj:04}.tif"
            files[name] = self.root / "version" / name
            ingest.write_tile(files[name], ti, tj, np.full((ingest.TILE_PX,) * 2, 9000, dtype))
        ids = [f"{ti:04}/{tj:04}" for ti, tj in tiles]
        facts = {key: {"product": key, "attribution": f"credit of {key}", "licence": "CC0",
                       "vertical_datum": "EGM2008", "fetched": "2026-10-24"} for key in ("es", "nl")}
        index = {"schema": 1, "step_log2": 6, "tile_log2": 16, "pool": pool, "sources": facts,
                 "tiles": {ids[0]: "nl", ids[1]: "es"}, "contributors": {ids[0]: ["nl", "es"], ids[1]: ["es"]},
                 "sha256": {ids[0]: "a", ids[1]: "b"}}
        files[f"{prefix}index.json"] = self.root / "version" / f"{prefix}index.json"
        files[f"{prefix}index.json"].write_text(json.dumps(index), encoding="utf-8")
        return files, ids

    def test_the_step_cuts_the_tiles_of_the_leaf_and_their_index(self):
        for pool in ("max", "mean"):
            files, ids = self.version(pool)
            output = self.root / pool / "output"
            output.mkdir(parents=True)
            options = {"source": "terrain", "tiles": [ids[1], "0000/0000"], **({"pool": pool} if pool == "mean" else {})}
            request = {"options": options, "snapshots": {"terrain": {name: str(path) for name, path in files.items()}},
                       "output": str(output)}
            self.assertEqual(merge.merge(request), {"tiles": 1, "sources": ["es"]})
            layer = output / "reference"
            index = json.loads((layer / "index.json").read_text(encoding="utf-8"))
            self.assertEqual((index["pool"], index["contributors"], list(index["sources"])), (pool, {ids[1]: ["es"]}, ["es"]))
            self.assertTrue(ingest.tile_path(layer, 4812, 4224).is_file())
            self.assertFalse(ingest.tile_path(layer, 4812, 4223).exists(), "a tile that the leaf does not read stays out")
            shutil.rmtree(self.root / "version")
