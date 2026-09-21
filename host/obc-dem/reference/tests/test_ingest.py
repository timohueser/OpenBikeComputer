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
import sys
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

import numpy as np
import rasterio
from rasterio.crs import CRS
from rasterio.transform import Affine
from rasterio.warp import transform_bounds

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import ingest  # noqa: E402

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


def bbox_of(path, pad=0.0005):
    with rasterio.open(path) as src:
        envelope = ingest.source_envelope(src.transform, src.width, src.height)
        west, south, east, north = transform_bounds(src.crs, ingest.WGS84, *envelope)
    return f"{west - pad},{south - pad},{east + pad},{north + pad}"


def archive_pixel(transform, row, col):
    """The archive tile and pixel that holds one source pixel's centre."""

    x, y = ingest.source_xy(transform, np.array([row + 0.5]), np.array([col + 0.5]))
    lon, lat = rasterio.warp.transform(LV95, ingest.WGS84, list(x), list(y))
    r = ingest.pixel_index(int(np.floor(lat[0] * ingest.DEGREE)))
    c = ingest.pixel_index(int(np.floor(lon[0] * ingest.DEGREE)))
    return ingest.tile_index(r), ingest.tile_index(c), r, c


def local_source(key, country="Testland"):
    return ingest.Source(key, country, f"{key} test product", 1.0, "CC0", f"© {key}", "EGM2008",
                         (-180, -90, 180, 90))


class ArchiveCase(unittest.TestCase):
    def setUp(self):
        self.work = TemporaryDirectory()
        self.root = Path(self.work.name)
        self.archive = self.root / "archive"
        self.inputs = self.root / "input"
        self.inputs.mkdir()
        self.addCleanup(self.work.cleanup)

    def ingest(self, key, raster, inputs=None):
        code = ingest.main(["ingest", key, "--bbox", bbox_of(raster),
                            "--archive", str(self.archive), "--input", str(inputs or self.inputs)])
        self.assertEqual(code, 0)

    def index(self):
        return json.loads((self.archive / "index.json").read_text(encoding="utf-8"))

    def only_tile(self):
        index = self.index()
        self.assertEqual(len(index["tiles"]), 1, index["tiles"])
        tile = next(iter(index["tiles"]))
        ti, tj = (int(part) for part in tile.split("/"))
        return tile, ingest.tile_path(self.archive, ti, tj)


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
            self.assertEqual(ingest.tile_problems(path, ti, tj), [])

    def test_a_box_over_the_antimeridian_or_the_pole_is_refused(self):
        for bbox in ("179.9,46.0,-179.9,46.1", "8.0,46.0,8.1,90.5"):
            with self.subTest(bbox=bbox), self.assertRaises(ingest.Refuse):
                ingest.check_world(ingest.parse_bbox(bbox), "--bbox")


class Ingest(ArchiveCase):
    def pool(self, values, name, transform=None, **kw):
        """Ingest one raster into an archive of its own, and return the single tile."""

        directory = self.root / name
        directory.mkdir()
        self.archive = self.root / f"archive-{name}"
        raster = source_raster(directory / f"{name}.tif", values, transform=transform, **kw)
        self.ingest("ch", raster, inputs=directory)
        tile, path = self.only_tile()
        with rasterio.open(path) as src:
            return tile, path, src.read(1)

    def test_the_tower_stays_one_pixel_wherever_it_stands(self):
        """The centre rule puts a one-pixel tower in exactly one archive pixel, always.

        Area-overlap resampling spreads it over two to four, because the archive pixel is
        about 7 m by 5 m and the tower's 1 m square touches more than one of them.
        """

        for row, col in ((50, 50), (51, 53), (7, 191), (123, 44), (0, 0), (199, 199)):
            with self.subTest(row=row, col=col):
                values = plateau_with_tower(row=row, col=col)
                tile, _, data = self.pool(values, f"t{row}x{col}")
                ti, tj, r, c = archive_pixel(grid(), row, col)
                self.assertEqual(tile, ingest.tile_id(ti, tj))
                self.assertEqual(int((data == round(TOWER)).sum()), 1)
                self.assertEqual(int(data[(ti + 1) * ingest.TILE_PX - 1 - r, c - tj * ingest.TILE_PX]),
                                 round(TOWER))

    def test_a_rotated_source_loses_nothing(self):
        """A centre is a point, so a 30° rotated grid pools like any other."""

        transform = grid(rotation=30.0)
        values = plateau_with_tower()
        tile, _, data = self.pool(values, "rotated", transform=transform)
        ti, tj, r, c = archive_pixel(transform, TOWER_ROW, TOWER_COL)
        self.assertEqual(tile, ingest.tile_id(ti, tj))
        self.assertEqual(int((data == round(TOWER)).sum()), 1)
        self.assertEqual(int(data[(ti + 1) * ingest.TILE_PX - 1 - r, c - tj * ingest.TILE_PX]), round(TOWER))
        self.assertEqual(int((data != ingest.NODATA).sum()), int((data == round(PLATEAU)).sum()) + 1)

    def test_the_tile_is_whole_and_holds_nothing_the_source_did_not_say(self):
        raster = source_raster(self.inputs / "tower.tif", plateau_with_tower())
        self.ingest("ch", raster)
        _, path = self.only_tile()
        with rasterio.open(path) as src:
            data = src.read(1)
        self.assertEqual(data.shape, (ingest.TILE_PX, ingest.TILE_PX))
        # A 200 m square covers about 28 x 41 pixels of the tile; the rest is nodata.
        covered = int((data != ingest.NODATA).sum())
        self.assertTrue(1000 <= covered <= 1400, covered)
        self.assertEqual(covered, int((data == round(PLATEAU)).sum()) + 1)

    def test_a_second_run_changes_nothing(self):
        raster = source_raster(self.inputs / "tower.tif", plateau_with_tower())
        self.ingest("ch", raster)
        first = self.index()
        before = ingest.tile_path(self.archive, *(int(p) for p in next(iter(first["tiles"])).split("/"))).read_bytes()
        self.ingest("ch", raster)
        tile, path = self.only_tile()
        self.assertEqual(self.index()["sha256"], first["sha256"])
        self.assertEqual(path.read_bytes(), before)

    def test_a_second_box_from_one_source_merges(self):
        """Two boxes into one tile keep both footprints and the higher of the overlap."""

        source_raster(self.inputs / "west.tif", np.full((SIDE, SIDE), 1000.0, dtype="float32"))
        self.ingest("ch", self.inputs / "west.tif")
        _, path = self.only_tile()
        with rasterio.open(path) as src:
            west_only = int((src.read(1) != ingest.NODATA).sum())

        east = self.root / "east"
        east.mkdir()
        source_raster(east / "east.tif", np.full((SIDE, SIDE), 1100.0, dtype="float32"),
                      transform=grid(east=EAST + 150))
        self.ingest("ch", east / "east.tif", inputs=east)
        with rasterio.open(path) as src:
            data = src.read(1)
        covered = int((data != ingest.NODATA).sum())
        self.assertGreater(covered, west_only)  # the second box added its own footprint
        self.assertGreater(int((data == 1000).sum()), 0)  # and did not drop the first
        self.assertLess(int((data == 1000).sum()), west_only)  # the 50 m overlap took the higher value
        self.assertEqual(covered, int((data == 1000).sum()) + int((data == 1100).sum()))

    def test_priority_decides_pixels_not_whole_tiles(self):
        """A better source wins the pixels it covers, and nothing else.

        Coverage stops at borders and survey edges, so a better source over one corner of a
        tile must not take the rest of the tile away from the source that does cover it.
        """

        ingest.SOURCES.update({"nl": local_source("nl"), "es": local_source("es")})
        self.addCleanup(lambda: [ingest.SOURCES.pop(key) for key in ("nl", "es")])
        coarse = source_raster(self.inputs / "coarse.tif", np.full((SIDE, SIDE), 1000.0, dtype="float32"))
        corner = self.root / "corner"
        corner.mkdir()
        # A quarter of the coarse source's square, in its north-west corner, reading lower.
        source_raster(corner / "corner.tif", np.full((SIDE // 2, SIDE // 2), 900.0, dtype="float32"),
                      transform=grid())

        self.ingest("es", coarse)
        tile, path = self.only_tile()
        self.assertEqual(self.index()["tiles"], {tile: "es"})
        with rasterio.open(path) as src:
            whole = src.read(1)
        covered = int((whole == 1000).sum())

        self.ingest("nl", corner / "corner.tif", inputs=corner)
        with rasterio.open(path) as src:
            data = src.read(1)
        self.assertGreater(int((data == 900).sum()), 0)  # the better source took its corner
        self.assertGreater(int((data == 1000).sum()), 0)  # and left the rest with `es`
        self.assertEqual(int((data == 900).sum()) + int((data == 1000).sum()), covered)
        self.assertEqual(self.index()["tiles"], {tile: "nl"})  # the best contributor
        self.assertEqual(sorted(self.index()["sources"]), ["es", "nl"])  # both attributions travel
        for key in ("es", "nl"):
            manifest = json.loads((self.archive / "sources" / f"{key}.json").read_text(encoding="utf-8"))
            self.assertEqual(manifest["tiles"], [tile])

        self.ingest("es", coarse)  # the coarser source cannot take the corner back
        with rasterio.open(path) as src:
            again = src.read(1)
        self.assertTrue((again == data).all())

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
                _, _, data = self.pool(values, name, dtype=dtype, nodata=nodata)
                self.assertEqual(int(data.max()), round(PLATEAU))
                self.assertNotIn(round(TOWER), set(np.unique(data).tolist()))

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

    def test_the_run_reports_the_void_fraction(self):
        values = plateau_with_tower()
        values[:100] = np.nan  # the top half of the raster is void
        raster = source_raster(self.inputs / "half.tif", values, nodata=np.nan)
        _, _, _, _, voided = ingest.read_source(raster)
        self.assertAlmostEqual(voided, 0.5, places=3)

    def test_a_source_coarser_than_the_step_invents_nothing(self):
        """A 20 m source reaches the lattice pixels its centres land in, and no others."""

        values = np.full((10, 10), 1234.0, dtype="float32")
        _, _, data = self.pool(values, "coarse", transform=grid(step=20.0))
        filled = int((data == 1234).sum())
        self.assertEqual(filled, 100)  # one lattice pixel per source pixel centre
        self.assertEqual(int(((data != 1234) & (data != ingest.NODATA)).sum()), 0)


class Check(ArchiveCase):
    def setUp(self):
        super().setUp()
        raster = source_raster(self.inputs / "tower.tif", plateau_with_tower())
        self.ingest("ch", raster)

    def test_check_passes_and_names_a_tile_the_index_forgot(self):
        self.assertEqual(ingest.main(["check", "--archive", str(self.archive)]), 0)
        index = self.index()
        index["tiles"] = {}
        index["sha256"] = {}
        (self.archive / "index.json").write_text(json.dumps(index), encoding="utf-8")
        self.assertEqual(ingest.main(["check", "--archive", str(self.archive)]), 1)

    def test_check_catches_a_tile_whose_pixels_moved(self):
        _, path = self.only_tile()
        with rasterio.open(path, "r+") as dst:
            dst.write(np.full((ingest.TILE_PX, ingest.TILE_PX), 42, dtype="int16"), 1)
        self.assertEqual(ingest.main(["check", "--archive", str(self.archive)]), 1)

    def test_check_holds_the_byte_layout_as_well(self):
        """A tile with the right pixels but the wrong layout is still not a tile."""

        tile, path = self.only_tile()
        ti, tj = (int(part) for part in tile.split("/"))
        with rasterio.open(path) as src:
            pixels, profile = src.read(1), src.profile
        with rasterio.open(path, "w", **{**profile, "compress": None, "tiled": False}) as dst:
            dst.write(pixels, 1)  # the same heights, with none of the layout the contract asks for
            dst.update_tags(AREA_OR_POINT="Point")
        problems = ingest.tile_problems(path, ti, tj)
        self.assertEqual(len(problems), 3, problems)
        for wanted in ("deflate", "(256, 256)", "AREA_OR_POINT"):
            self.assertTrue(any(wanted in problem for problem in problems), problems)
        # The digest is over the pixels, so it did not move with the layout.
        self.assertEqual(ingest.tile_digest(path), self.index()["sha256"][tile])

    def test_the_digest_is_the_pixel_array(self):
        tile, path = self.only_tile()
        with rasterio.open(path) as src:
            pixels = src.read(1)
        wanted = hashlib.sha256(pixels.astype("<i2").tobytes()).hexdigest()
        self.assertEqual(self.index()["sha256"][tile], wanted)

    def test_check_names_a_file_that_is_not_a_tile_id(self):
        stray = self.archive / "16" / "nine" / "0001.tif"
        stray.parent.mkdir(parents=True)
        stray.write_bytes(b"")
        self.assertEqual(ingest.main(["check", "--archive", str(self.archive)]), 1)

    def test_index_rebuilds_from_the_manifest(self):
        manifest = json.loads((self.archive / "sources" / "ch.json").read_text(encoding="utf-8"))
        self.assertEqual(manifest["product"], "swissALTI3D 2 m")
        self.assertEqual(manifest["attribution"], "© swisstopo")
        (self.archive / "index.json").unlink()
        self.assertEqual(ingest.main(["index", "--archive", str(self.archive)]), 0)
        index = self.index()
        self.assertEqual(index["schema"], 1)
        self.assertEqual((index["step_log2"], index["tile_log2"]), (6, 16))
        self.assertEqual(list(index["tiles"].values()), ["ch"])
        self.assertEqual(index["sources"]["ch"]["licence"], "Open data, attribution required")
        self.assertEqual(index["sources"]["ch"]["vertical_datum"], "LN02/LHN95")
        self.assertEqual(manifest["vertical_datum"], "LN02/LHN95")
        self.assertEqual(sorted(index["sha256"]), sorted(index["tiles"]))


class RcloneSeam(ArchiveCase):
    """The publish and mirror plans, with rclone replaced by a recorder.

    rclone is not a test dependency, so the tests hold the three things this tool owns: the
    order the objects reach R2 in, the fact that a publish only ever adds, and the fact that
    the secret travels in the child's environment and never in argv.
    """

    def setUp(self):
        super().setUp()
        raster = source_raster(self.inputs / "tower.tif", plateau_with_tower())
        self.ingest("ch", raster)
        self.calls = []
        for name, value in (
            ("OBC_R2_ACCOUNT_ID", "acc"), ("OBC_R2_BUCKET", "maps"), ("OBC_R2_PREFIX", "/obc/"),
            ("OBC_R2_ACCESS_KEY_ID", "key"), ("OBC_R2_SECRET_ACCESS_KEY", "s3cret"),
        ):
            previous = os.environ.get(name)
            os.environ[name] = value
            self.addCleanup(lambda n=name, p=previous: os.environ.__setitem__(n, p) if p else os.environ.pop(n, None))
        real = ingest.run_rclone
        ingest.run_rclone = self.record
        self.addCleanup(lambda: setattr(ingest, "run_rclone", real))

    #: What the fake R2 already holds: another region's tile, from another source.
    PUBLISHED = {
        "schema": 1, "step_log2": 6, "tile_log2": 16,
        "sources": {"es": {"product": "MDT05", "attribution": "© IGN", "licence": "CC BY 4.0",
                           "fetched": "2026-01-01"}},
        "tiles": {"0100/0200": "es"},
        "sha256": {"0100/0200": "aa" * 32},
    }

    #: What `rclone lsf` answers: an empty line means R2 holds no index yet.
    LSF = "index.json\n"

    def record(self, argv, env, capture=False):
        self.calls.append((argv, env))
        if argv[0] == "lsf":
            return self.LSF
        if argv[0] == "copyto":  # the mirror reads the index it just pulled
            Path(argv[2]).write_text((self.archive / "index.json").read_text(encoding="utf-8"), encoding="utf-8")
        if "--include" in argv:  # the publish pulls the index that is already on R2
            Path(argv[2], "index.json").write_text(json.dumps(self.PUBLISHED), encoding="utf-8")
        elif argv[0] == "copy" and argv[1].endswith("index.json"):
            self.uploaded = json.loads(Path(argv[1]).read_text(encoding="utf-8"))
        if "--files-from" in argv:
            self.listing = Path(argv[argv.index("--files-from") + 1]).read_text(encoding="utf-8").split()

    def test_publish_adds_the_tiles_and_sends_a_merged_index_last(self):
        """Nothing is deleted, and the index that goes up names R2's tiles as well as ours."""

        self.assertEqual(ingest.main(["publish", "--archive", str(self.archive)]), 0)
        upload, listing, fetch, publish = (argv for argv, _ in self.calls)
        self.assertEqual([argv[0] for argv in (upload, fetch, publish)], ["copy", "copy", "copy"])
        self.assertEqual(listing[0], "lsf")
        self.assertNotIn("--delete", [word for argv, _ in self.calls for word in argv])

        self.assertEqual(upload[1:3], [str(self.archive), "OBCR2:maps/obc/reference/v1"])
        self.assertEqual(upload[upload.index("--exclude") + 1], "/index.json")
        self.assertEqual(fetch[1], "OBCR2:maps/obc/reference/v1")  # the index R2 already has
        self.assertEqual(publish[2], "OBCR2:maps/obc/reference/v1")
        self.assertTrue(publish[1].endswith("index.json"), publish)
        self.assertNotEqual(Path(publish[1]).parent, self.archive)  # the merge is not the local index

        mine = next(iter(self.index()["tiles"]))
        self.assertEqual(self.uploaded["tiles"], {"0100/0200": "es", mine: "ch"})
        self.assertEqual(sorted(self.uploaded["sources"]), ["ch", "es"])
        self.assertEqual(sorted(self.uploaded["sha256"]), sorted(self.uploaded["tiles"]))
        self.assertEqual(self.uploaded["sha256"][mine], self.index()["sha256"][mine])

    def test_a_first_publish_does_not_fetch_an_index_that_is_not_there(self):
        """An empty `lsf` is how an empty bucket is told from one with an index in it."""

        self.LSF = ""
        self.assertEqual(ingest.main(["publish", "--archive", str(self.archive)]), 0)
        self.assertEqual([argv[0] for argv, _ in self.calls], ["copy", "lsf", "copy"])
        self.assertEqual(self.uploaded["tiles"], self.index()["tiles"])

    def test_this_archive_wins_a_tile_r2_also_holds(self):
        mine = next(iter(self.index()["tiles"]))
        published = {**self.PUBLISHED, "tiles": {**self.PUBLISHED["tiles"], mine: "es"},
                     "sha256": {**self.PUBLISHED["sha256"], mine: "bb" * 32}}
        merged = ingest.merge_index(published, self.index())
        self.assertEqual(merged["tiles"][mine], "ch")
        self.assertEqual(merged["sha256"][mine], self.index()["sha256"][mine])

    def test_a_published_index_of_another_contract_is_refused(self):
        with self.assertRaises(ingest.Refuse):
            ingest.merge_index({**self.PUBLISHED, "step_log2": 5}, self.index())
        self.assertEqual(ingest.merge_index(None, self.index()), self.index())

    def test_the_secret_is_in_the_environment_only(self):
        ingest.main(["publish", "--archive", str(self.archive)])
        argv, env = self.calls[0]
        self.assertEqual(env["RCLONE_CONFIG_OBCR2_SECRET_ACCESS_KEY"], "s3cret")
        self.assertEqual(env["RCLONE_CONFIG_OBCR2_ENDPOINT"], "https://acc.r2.cloudflarestorage.com")
        self.assertNotIn("s3cret", " ".join(argv))

    def test_mirror_asks_for_the_tiles_the_box_needs(self):
        target = self.root / "mirror"
        bbox = bbox_of(self.inputs / "tower.tif")
        self.assertEqual(ingest.main(["mirror", "--archive", str(target), "--bbox", bbox]), 0)
        copyto, copy = (argv for argv, _ in self.calls)
        self.assertEqual(copyto[1], "OBCR2:maps/obc/reference/v1/index.json")
        # The box needs its own tile and the ring around it; only the one is in the index.
        self.assertGreaterEqual(len(ingest.box_tiles(ingest.parse_bbox(bbox))), 9)
        self.assertEqual(self.listing, [f"16/{next(iter(self.index()['tiles']))}.tif"])
        wanted = Path(copy[copy.index("--files-from") + 1])
        self.assertFalse(wanted.exists())  # the listing is temporary, not archive content

    def test_the_mirror_halo_is_one_tile_on_every_side(self):
        tile = next(iter(self.index()["tiles"]))
        ti, tj = (int(part) for part in tile.split("/"))
        needed = ingest.box_tiles(ingest.parse_bbox(bbox_of(self.inputs / "tower.tif")))
        for di in (-1, 0, 1):
            for dj in (-1, 0, 1):
                self.assertIn(ingest.tile_id(ti + di, tj + dj), needed)


if __name__ == "__main__":
    unittest.main()
