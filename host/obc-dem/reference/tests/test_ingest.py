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
import ingest.cli  # noqa: E402
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


def window(ti, tj, bi=1203, bj=1055):
    """Tile `ti`, `tj` in the north-up array of block `bi`, `bj`, by the formula of the spec: the
    tile starts (4 bi + 3 - ti) x 1024 rows below the north edge and (tj - 4 bj) x 1024 columns east
    of the west edge."""

    row, col = (4 * bi + 3 - ti) * ingest.TILE_PX, (tj - 4 * bj) * ingest.TILE_PX
    return np.s_[row:row + ingest.TILE_PX, col:col + ingest.TILE_PX]


def block_problems(path, bi, bj, pool="max"):
    """`tile_problems` of a block of a prepared version: 4 x 4 tiles in one raster."""

    with rasterio.open(path) as src:
        size = (src.width, src.height)
        moved = max(abs(a - b) for a, b in zip(src.transform[:6], ingest.block_window(bi, bj).transform[:6]))
    problems = [f"size: {size}"] if size != (ingest.BLOCK_PX,) * 2 else []
    problems += [f"transform is {moved} degrees off the lattice"] if moved > 1e-9 else []
    rest = tile_problems(path, 4 * bi, 4 * bj, pool)
    return problems + [line for line in rest if not line.startswith(("size", "transform"))]


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
        collapsed onto one lattice line. The pooling alone shows it; the fill then fills the
        pixels between the centres.
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
        values, src_transform, src_crs, bounds, _ = ingest.read_source(raster)
        window = ingest.covering_window(bounds, pad=1)
        high, _, count, dropped = ingest.pool_onto_lattice(values, src_transform, src_crs, window)
        self.assertEqual((int((count > 0).sum()), int(count.max()), dropped), (100 * 100, 1, 0))
        self.assertEqual(int((high == np.float32(TOWER)).sum()), 1)
        ti, tj, r, c = at(lon_u, lat_u)
        data, _ = ingest.pool_tile([raster], ingest.SOURCES["ch"], ti, tj)
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

    def test_a_source_coarser_than_the_step_fills_its_footprint(self):
        """A 20 m source keeps the pixels its centres land in, and fills each other pixel between
        its centres with their bilinear height, which on a plane is the plane. Nothing outside."""

        plane = lambda x, y: 1000.0 + 0.3 * (x - EAST) - 0.2 * (y - NORTH)  # noqa: E731
        transform = grid(step=20.0)
        xs, ys = ingest.source_xy(transform, *np.mgrid[0:10, 0:10] + 0.5)
        raster = source_raster(self.root / "coarse.tif", plane(xs, ys), transform=transform)
        ti, tj, _, _ = archive_pixel(transform, 5, 5)
        source = ingest.Source("xx", "Testland", "xx test product", 20.0, "CC0", "EGM2008")
        maximum, mean = ingest.pool_tile([raster], source, ti, tj)

        reached = np.zeros(maximum.shape, dtype=bool)
        for row in range(10):
            for col in range(10):
                reached[archive_pixel(transform, row, col)[2:]] = True
        rows, cols = np.mgrid[0:ingest.TILE_PX, 0:ingest.TILE_PX]
        lat = (ingest.GRID_ORIGIN + ((ti + 1) * ingest.TILE_PX - 1 - rows) * ingest.STEP + ingest.STEP // 2) / 1e6
        lon = (ingest.GRID_ORIGIN + (tj * ingest.TILE_PX + cols) * ingest.STEP + ingest.STEP // 2) / 1e6
        x, y = (np.asarray(v).reshape(lat.shape) for v in rasterio.warp.transform(
            ingest.WGS84, LV95, lon.ravel(), lat.ravel()))
        between = (x > EAST + 10) & (x < EAST + 190) & (y < NORTH - 10) & (y > NORTH - 190)
        np.testing.assert_array_equal(maximum != ingest.NODATA, between | reached)
        filled = between & ~reached
        self.assertGreater(int(filled.sum()), int(reached.sum()))
        self.assertLess(float(np.abs(mean[filled] / 10.0 - plane(x[filled], y[filled])).max()), 0.06)


    def test_a_raster_of_one_row_or_column_pools_its_centres_and_fills_nothing(self):
        """A box clipped to the extent of a model can give a sliver of one source row."""

        for shape in ((1, 10), (10, 1)):
            with self.subTest(shape=shape):
                raster = source_raster(self.root / "sliver.tif", np.full(shape, 1234.0, dtype="float32"),
                                       transform=grid(step=20.0))
                ti, tj, _, _ = archive_pixel(grid(step=20.0), 0, 0)
                maximum, _ = ingest.pool_tile([raster], ingest.SOURCES["ch"], ti, tj)
                self.assertEqual(int((maximum == 1234).sum()), 10)
                self.assertEqual(int((maximum != ingest.NODATA).sum()), 10)

    def test_only_a_model_with_a_step_longer_than_the_east_side_of_a_pixel_is_coarse(self):
        """Such a model is fetched with a margin: `it` and `is` at 10 m, and not `es` at 5 m, whose
        country is south of the latitude where the east side of a pixel is 5 m."""

        tile = lambda lat, lon: ((int(lat * 1e6) - ingest.GRID_ORIGIN) >> 16, (int(lon * 1e6) - ingest.GRID_ORIGIN) >> 16)  # noqa: E731
        for key, (lat, lon), coarse in (("it", (42.0, 12.5), True), ("is", (64.5, -19.0), True),
                                        ("es", (43.5, -5.0), False), ("ch", (46.8, 8.2), False)):
            with self.subTest(key):
                self.assertEqual(ingest.archive.coarse(ingest.SOURCES[key], *tile(lat, lon)), coarse)

    def test_the_bias_samples_are_the_mean_at_every_64th_pixel_row_by_row_from_the_south(self):
        """An asymmetric tile, so a flipped or transposed mark fails."""

        rows, cols = np.mgrid[0:ingest.TILE_PX, 0:ingest.TILE_PX]
        mean = ((ingest.TILE_PX - 1 - rows) * 10_000 + cols).astype("int32")  # north-up: row 0 is north
        mean[ingest.TILE_PX - 1 - 32, 96] = ingest.MEAN_NODATA
        lines = [line.split() for line in ingest.archive.samples(mean).splitlines()]
        self.assertEqual([len(line) for line in lines], [16] * 16)
        self.assertEqual(lines[0][:3], [str(32 * 10_000 + 32), "-", str(32 * 10_000 + 160)])
        self.assertEqual(lines[15][15], str(992 * 10_000 + 992))
        self.assertEqual(lines[1][0], str(96 * 10_000 + 32))


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
    """`ingest.py fetch`: one block of a model, pooled, and nothing of the raw rasters."""

    def fetch(self, key, *tiles, block=None):
        """`ingest.py fetch` of `key` in `tiles` of their block: its exit code and the files it wrote."""

        out = self.root / "out"
        shutil.rmtree(out, ignore_errors=True)
        if block is None:
            ti, tj = (int(part) for part in tiles[0].split("-"))
            block = f"{ti // 4:04}-{tj // 4:04}"
        code = ingest.main(["fetch", *(f"{tile}={key}" for tile in tiles), "--block", block,
                            "--work", str(self.root / "work"), "--out", str(out)])
        return code, sorted(str(path.relative_to(out)) for path in out.rglob("*") if path.is_file())

    def serve(self, service):
        real = ingest.SOURCES["ch"]
        ingest.SOURCES["ch"] = service
        self.addCleanup(ingest.SOURCES.__setitem__, "ch", real)
        return service

    def test_a_block_holds_what_the_service_answered_for_each_tile_and_the_raw_rasters_are_removed(self):
        # A 32 µdeg source that straddles two tiles of block 1203-1055, with a tower in each. Its
        # columns 0 to 99 land in the west tile and 100 to 199 in the east one, four centres in a
        # lattice pixel.
        transform, _, _ = wgs84_grid(32, tj=4222, col=ingest.TILE_PX - 50)
        values = np.full((200, 200), 1000.0, dtype="float32")
        values[100, 20] = values[100, 140] = TOWER
        raster = source_raster(self.inputs / "wide.tif", values, transform=transform)
        with rasterio.open(raster, "r+") as dst:
            dst.crs = ingest.WGS84
        service = self.serve(BoxService(raster))
        files = ["1203-1055.mean.sha256", "1203-1055.sha256", "1203-1055.tiles", "18/1203/1055.tif",
                 "mean/18/1203/1055.tif"]
        self.assertEqual(self.fetch("ch", "4812-4222", "4812-4223"), (0, files))
        self.assertEqual(service.boxes, [ingest.tile_bounds(4812, 4222), ingest.tile_bounds(4812, 4223)])
        self.assertFalse((self.root / "work").exists())
        out = self.root / "out"
        for pool, root in (("max", out), (ingest.MEAN, out / ingest.MEAN)):
            self.assertEqual(block_problems(ingest.block_path(root, 1203, 1055), 1203, 1055, pool), [])
        with rasterio.open(ingest.block_path(out, 1203, 1055)) as src:
            block = src.read(1)
        self.assertEqual((out / "1203-1055.sha256").read_text(encoding="utf-8"), ingest.pixel_digest(block))
        marks = json.loads((out / "1203-1055.tiles").read_text(encoding="utf-8"))
        self.assertEqual(sorted(marks), ["4812/4222", "4812/4223"])
        for tj in (4222, 4223):
            data = block[window(4812, tj)]
            self.assertEqual((int((data != ingest.NODATA).sum()), int((data == round(TOWER)).sum())), (50 * 100, 1))
            mark = marks[f"4812/{tj}"]
            self.assertEqual(mark["sources"], ["ch"])
            marked = mark["samples"].split()
            self.assertEqual(len(marked), 256)
            self.assertEqual({value for value in marked if value != "-"}, {"10000"}, "1000 m in decimetres")
        self.assertTrue((block[window(4813, 4222)] == ingest.NODATA).all(), "a tile of no model")

        index, keys, blocks = self.root / "index.json", self.root / "keys.json", self.root / "blocks.json"
        biases = self.root / "biases.json"
        tiles = {"4812/4222": ["ch"], "4812/4223": ["ch"]}
        keys.write_text(json.dumps(tiles), encoding="utf-8")
        blocks.write_text(json.dumps(["1203/1055"]), encoding="utf-8")
        biases.write_text(json.dumps({"ch": -1.25}), encoding="utf-8")
        self.assertEqual(ingest.main(["index-terrain", "--fetched", "2026-10-01", "--keys", str(keys),
                                      "--blocks", str(blocks), "--models", "ch", "--biases", str(biases),
                                      "--out", str(index)]), 0)
        text = index.read_text(encoding="utf-8")
        index = json.loads(text)
        self.assertEqual(sorted(index), ["block_log2", "contributors", "sources", "step_log2", "tile_log2"])
        self.assertEqual(text, json.dumps(index, sort_keys=True, separators=(",", ":"), ensure_ascii=False))
        self.assertEqual((index["contributors"], index["sources"]["ch"]["fetched"]), (tiles, "2026-10-01"))
        self.assertEqual(index["sources"]["ch"]["glo30_bias_m"], -1.25)

        # A next version keeps the tiles of the other blocks and the facts of a model that it did not
        # fetch, its bias too. The tiles of a block that it made again are those of its marks.
        before = self.root / "before.json"
        before.write_text(json.dumps({**index, "contributors": {**tiles, "4800/4000": ["ch"]}}), encoding="utf-8")
        later = self.root / "later.json"
        keys.write_text(json.dumps({"4812/4222": ["nl", "ch"]}), encoding="utf-8")
        biases.write_text(json.dumps({"nl": 0.5}), encoding="utf-8")
        self.stand_in("nl")
        self.assertEqual(ingest.main(["index-terrain", "--base", str(before), "--fetched", "2026-10-24",
                                      "--keys", str(keys), "--blocks", str(blocks), "--models", "nl",
                                      "--biases", str(biases), "--out", str(later)]), 0)
        later = json.loads(later.read_text(encoding="utf-8"))
        self.assertEqual(later["contributors"], {"4800/4000": ["ch"], "4812/4222": ["nl", "ch"]})
        self.assertEqual((later["sources"]["ch"]["fetched"], later["sources"]["nl"]["fetched"]),
                         ("2026-10-01", "2026-10-24"))
        self.assertEqual((later["sources"]["ch"]["glo30_bias_m"], later["sources"]["nl"]["glo30_bias_m"]), (-1.25, 0.5))

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
        self.assertEqual(self.fetch("ch", "4823-4256"), (0, ["1205-1064.none"]))
        west, south, _, north = ingest.tile_bounds(4823, 4256)
        self.assertEqual(Service.boxes, [(west, south, 10.5, north)])
        self.assertEqual(self.fetch("ch", "4823-4300"), (0, ["1205-1075.none"]))
        self.assertEqual(len(Service.boxes), 1, "a tile outside the extent asks nothing")
        self.assertEqual(self.fetch("ch", "4823-4255"), (1, []))
        self.assertFalse((self.root / "work").exists())
        self.assertEqual(self.fetch("ch", "48230-4255", block="1205-1063")[0], 1)
        self.assertEqual(self.fetch("ch", "4823-4255", block="1205-1064")[0], 1, "a tile of another block")

    def test_a_delivery_is_fetched_from_the_directory_that_the_environment_names(self):
        """A row without a service reads its delivery like a credential, and leaves it as it is."""

        (self.inputs / "order").mkdir()
        raster = source_raster(self.inputs / "order" / "dem.tif", plateau_with_tower())
        ti, tj, _, _ = archive_pixel(grid(), TOWER_ROW, TOWER_COL)
        name = f"{ti:04}-{tj:04}"
        with unittest.mock.patch.dict(os.environ, {"OBC_REFERENCE_AU_INPUT": str(self.inputs)}):
            self.assertEqual(self.fetch("au", name)[0], 1, "the datum of the order is not confirmed")
            os.environ["OBC_REFERENCE_AU_DATUM"] = "AHD"
            block, path = f"{ti // 4:04}-{tj // 4:04}", f"18/{ti // 4:04}/{tj // 4:04}.tif"
            files = [f"{block}.mean.sha256", f"{block}.sha256", f"{block}.tiles", path, f"mean/{path}"]
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
    """The models of a tile, folded worst first."""

    def test_the_best_model_with_a_height_holds_each_pixel_whatever_order_they_come_in(self):
        """`data/sources.toml` lists `fr` before `it` before `it-21`. Folded in that order, TINITALY
        would keep the pixels that Piedmont has. A model whose pixels the better ones all take is
        no source of the tile."""

        nodata, mean_nodata = ingest.NODATA, ingest.MEAN_NODATA
        heights = {"fr": (1, nodata, nodata, nodata), "it-21": (nodata, 2, nodata, nodata), "it": (3, 3, 3, nodata),
                   "es": (4, 4, nodata, nodata)}

        def pooled(source, ti, tj, work, shared):
            row = np.full((ingest.TILE_PX, ingest.TILE_PX), nodata, dtype="int16")
            row[0, :4] = heights[source.key]
            mean = np.where(row == nodata, mean_nodata, row.astype("int32") * 10).astype("int32")
            return row, mean

        with TemporaryDirectory() as directory, unittest.mock.patch.object(ingest.cli, "pool_model", pooled):
            out = Path(directory) / "out"
            self.assertEqual(ingest.main(["fetch", "4812-4223=fr,it,es,it-21", "--block", "1203-1055",
                                          "--work", str(Path(directory) / "work"), "--out", str(out)]), 0)
            with rasterio.open(ingest.block_path(out, 1203, 1055)) as src:
                self.assertEqual(src.read(1)[window(4812, 4223)][0, :4].tolist(), [1, 2, 3, nodata])
            marks = json.loads((out / "1203-1055.tiles").read_text(encoding="utf-8"))
            self.assertEqual(marks["4812/4223"]["sources"], ["fr", "it-21", "it"])


class Summit(unittest.TestCase):
    """A row's summit: a model that misses it is refused before its heights are used."""

    class Peak(ingest.Source):
        """A 10 m raster in degrees around (8.0, 46.5), inside the extent of `dtm-ch`, with one peak."""

        fetches = 0

        def fetch(self, bbox, workdir):
            type(self).fetches += 1
            workdir.mkdir(parents=True, exist_ok=True)
            values = np.full((100, 100), 1000.0, dtype="float32")
            values[50, 50] = self.peak
            path = workdir / "peak.tif"
            profile = {"driver": "GTiff", "height": 100, "width": 100, "count": 1, "dtype": "float32",
                       "crs": ingest.WGS84, "transform": Affine(1e-4, 0, 7.995, 0, -1e-4, 46.505)}
            with rasterio.open(path, "w", **profile) as dst:
                dst.write(values, 1)
            return [path]

    def model(self, peak, summit=(8.00005, 46.49995)):
        source = self.Peak("ch", "Testland", "test", 10.0, "CC0", None, summit=("Testhorn", *summit, 2000))
        source.peak = peak
        return source

    def test_a_model_within_the_tolerance_passes_once_per_prepare_and_one_beside_it_is_refused(self):
        with TemporaryDirectory() as directory:
            shared = Path(directory)
            ingest.cli.check_summit(self.model(2004.0), shared)
            ingest.cli.check_summit(self.model(2004.0), shared)
            self.assertEqual(self.Peak.fetches, 1, "the mark of the passed check answers the second tile")
            self.assertFalse((shared / "checks" / "ch").exists(), "the rasters of the check are removed")
        self.assertEqual(self.model(2004.0).vertical_datum, "orthometric, checked at Testhorn (2000 m)")
        # A summit point a few hundred metres off still finds the peak.
        with TemporaryDirectory() as directory:
            ingest.cli.check_summit(self.model(2004.0, summit=(8.0035, 46.4975)), Path(directory))
        with TemporaryDirectory() as directory, self.assertRaises(ingest.Refuse) as refusal:
            ingest.cli.check_summit(self.model(2055.0), Path(directory))
        self.assertIn("2055", str(refusal.exception))
        with TemporaryDirectory() as directory, self.assertRaises(ingest.Refuse) as refusal:
            ingest.cli.check_summit(self.model(2004.0, summit=(8.1, 46.6)), Path(directory))
        self.assertIn("no height", str(refusal.exception))

    def test_a_datum_that_names_no_known_system_needs_a_summit(self):
        with self.assertRaises(ingest.Refuse):
            ingest.Source("x", "Nowhere", "p", 1.0, "l", "orthometric, unverified")


class MergeStep(ArchiveCase):
    """The step of a map leaf: the tiles of a version of `terrain` that it reads, and their index."""

    def version(self, pool):
        """A version of `terrain` with one block of `pool` that holds two tiles: its files by name, the
        tile ids and the pixels of the block."""

        prefix, dtype = ("mean/", "int32") if pool == "mean" else ("", "int16")
        name = f"{prefix}18/1203/1055.tif"
        files = {name: self.root / "version" / name}
        pixels = (np.arange(ingest.BLOCK_PX ** 2) % 9000).astype(dtype).reshape(ingest.BLOCK_PX, ingest.BLOCK_PX)
        ingest.write_raster(files[name], ingest.block_window(1203, 1055), pixels)
        ids = ["4812/4222", "4812/4223"]
        facts = {key: {"product": key, "attribution": f"credit of {key}", "licence": "CC0",
                       "vertical_datum": "EGM2008", "fetched": "2026-10-24"} for key in ("es", "nl")}
        index = {"block_log2": 18, "step_log2": 6, "tile_log2": 16, "sources": facts,
                 "contributors": {ids[0]: ["nl", "es"], ids[1]: ["es"]}}
        files["index.json"] = self.root / "version" / "index.json"
        files["index.json"].write_text(json.dumps(index), encoding="utf-8")
        return files, ids, pixels

    def test_the_step_cuts_the_tiles_of_the_leaf_and_their_index(self):
        for pool in ("max", "mean"):
            files, ids, pixels = self.version(pool)
            output = self.root / pool / "output"
            output.mkdir(parents=True)
            options = {"source": "terrain", "tiles": [ids[1], "4813/4222", "0000/0000"],
                       **({"pool": pool} if pool == "mean" else {})}
            request = {"options": options, "snapshots": {"terrain": {name: str(path) for name, path in files.items()}},
                       "output": str(output)}
            self.assertEqual(merge.merge(request), {"tiles": 1, "sources": ["es"]})
            layer = output / "reference"
            index = json.loads((layer / "index.json").read_text(encoding="utf-8"))
            self.assertEqual((index["pool"], index["contributors"], list(index["sources"])),
                             (pool, {ids[1]: ["es"]}, ["es"]))
            tile = ingest.tile_path(layer, 4812, 4223)
            self.assertEqual(tile_problems(tile, 4812, 4223, pool), [])
            with rasterio.open(tile) as src:
                self.assertTrue((src.read(1) == pixels[window(4812, 4223)]).all(), "the window of the tile")
            self.assertEqual((index["tiles"], index["sha256"]), ({ids[1]: "es"}, {ids[1]: ingest.tile_digest(tile)}))
            self.assertFalse(ingest.tile_path(layer, 4812, 4222).exists(), "a tile that the leaf does not read stays out")
            shutil.rmtree(self.root / "version")
