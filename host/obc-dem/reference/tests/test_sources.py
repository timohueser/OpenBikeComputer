"""What each adapter asks a service for, and what it does with the answer.

No test here reaches the network. A request is held as the URL it would send, and the one
adapter that parses bytes itself — France, which is served raw float32 over WMS — is held
against a synthetic answer. The live probes are in `README.md`, with the summit each
source was verified against.
"""

import sys
import unittest
import urllib.parse
import zipfile
from pathlib import Path
from tempfile import TemporaryDirectory

import numpy as np
import rasterio
from pyproj import Transformer
from rasterio.crs import CRS
from rasterio.transform import Affine

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import ingest  # noqa: E402
import ingest.sources.base  # noqa: E402
import ingest.sources.bulk  # noqa: E402
import ingest.sources.cog  # noqa: E402
import ingest.sources.stac  # noqa: E402
from ingest.sources import fr as fr_module  # noqa: E402
from ingest.sources.grid import grid_squares  # noqa: E402
from ingest.sources.protocols import MAX_PIXELS, request_boxes  # noqa: E402

BOX = (6.8255, 45.9257, 6.8495, 45.9417)  # about 1.9 x 1.8 km over Chamonix


class TempCase(unittest.TestCase):
    """A test that needs somewhere to write rasters."""

    def setUp(self):
        self.work = TemporaryDirectory()
        self.root = Path(self.work.name)
        self.inputs = self.root / "input"
        self.inputs.mkdir()
        self.addCleanup(self.work.cleanup)


def query(url):
    """A service URL as its path and a case-folded dict of its query."""

    parts = urllib.parse.urlsplit(url)
    values = urllib.parse.parse_qs(parts.query, keep_blank_values=True)
    return (f"{parts.scheme}://{parts.netloc}{parts.path}",
            {key.lower(): value for key, value in values.items()})


class Requests(unittest.TestCase):
    """Every live adapter's request, held as the URL it would send."""

    def test_the_arcgis_request_asks_for_float32_metres_in_wgs84(self):
        """3DEP resamples to the size it is asked for, so the size and the type are stated."""

        path, values = query(ingest.SOURCES["us"].url(BOX))
        self.assertTrue(path.endswith("/3DEPElevation/ImageServer/exportImage"))
        self.assertEqual(values["bbox"], [",".join(str(value) for value in BOX)])
        self.assertEqual((values["bboxsr"], values["imagesr"]), (["4326"], ["4326"]))
        self.assertEqual((values["format"], values["pixeltype"], values["f"]),
                         (["tiff"], ["F32"], ["image"]))
        px, py = (int(value) for value in values["size"][0].split(","))
        # About 1.9 km by 1.8 km at the product's 1 m step, and never over the cap.
        self.assertEqual((px, py), (1858, 1781))
        self.assertLessEqual(max(px, py), MAX_PIXELS)

    def test_a_wcs_20_request_names_the_coverage_its_axes_and_the_output_size(self):
        """WCS 2.0 answers at the native step unless asked, and a label the server does not
        know is a refusal, so both the axes and the size are in every request."""

        for key, coverage, epsg, axes in (("es", "Elevacion4258_5", 4326, ("long", "lat")),
                                          ("nl", "dtm_05m", 28992, ("x", "y")),
                                          ("de-nw", "nw_dgm", 25832, ("x", "y"))):
            with self.subTest(key):
                source = ingest.SOURCES[key]
                _, values = query(source.url(BOX))
                self.assertEqual(values["version"], ["2.0.1"])
                self.assertEqual(values["request"], ["GetCoverage"])
                self.assertEqual(values["coverageid"], [coverage])
                self.assertEqual(values["format"], ["image/tiff"])
                self.assertEqual(source.epsg, epsg)

                subsets = sorted(values["subset"])
                self.assertEqual([value.split("(")[0] for value in subsets], sorted(axes))
                scale = dict(part.split("(") for part in values["scalesize"][0].split(","))
                self.assertEqual(sorted(scale), sorted(axes))
                for size in scale.values():
                    self.assertLessEqual(int(size.rstrip(")")), MAX_PIXELS)

    def test_a_projected_wcs_request_encloses_the_box_in_the_services_own_grid(self):
        """The subsets are metres on the national grid, and they enclose the box.

        A projected grid's axes are not the box's, so the request is the rectangle that
        contains all four transformed corners. It reaches a little past the box, which is
        what keeps a curved projection edge from cutting a corner off.
        """

        source = ingest.SOURCES["de-nw"]
        _, values = query(source.url(BOX))
        subsets = {part.split("(")[0]: part.split("(")[1].rstrip(")").split(",")
                   for part in values["subset"]}
        to_grid = Transformer.from_crs("EPSG:4326", f"EPSG:{source.epsg}", always_xy=True)
        for lon, lat in ((BOX[0], BOX[1]), (BOX[2], BOX[1]), (BOX[0], BOX[3]), (BOX[2], BOX[3])):
            x, y = to_grid.transform(lon, lat)
            self.assertLessEqual(float(subsets["x"][0]), x)
            self.assertGreaterEqual(float(subsets["x"][1]), x)
            self.assertLessEqual(float(subsets["y"][0]), y)
            self.assertGreaterEqual(float(subsets["y"][1]), y)

    def test_the_wcs_10_request_states_a_bbox_with_a_width_and_a_height(self):
        """Norway answers WCS 1.0.0, which sizes its grid differently from 2.0.1."""

        source = ingest.SOURCES["no"]
        _, values = query(source.url(BOX))
        self.assertEqual(values["version"], ["1.0.0"])
        self.assertEqual(values["coverage"], ["nhm_dtm_topo_25833"])
        self.assertEqual(values["crs"], ["EPSG:25833"])
        self.assertEqual(values["format"], ["GeoTIFF"])
        self.assertNotIn("subset", values)
        self.assertEqual(len(values["bbox"][0].split(",")), 4)
        self.assertLessEqual(max(int(values["width"][0]), int(values["height"][0])), MAX_PIXELS)

    def test_a_box_larger_than_the_cap_becomes_several_requests(self):
        """Half-metre LiDAR overruns every server's size cap, so the box is cut up."""

        wide = (6.0, 46.0, 6.2, 46.2)  # about 15 km by 22 km
        for key in ("us", "nl"):
            with self.subTest(key):
                source = ingest.SOURCES[key]
                boxes = list(request_boxes(wide, source.resolution_m))
                self.assertGreater(len(boxes), 1)
                self.assertAlmostEqual(min(box[0] for box in boxes), wide[0])
                self.assertAlmostEqual(max(box[3] for box in boxes), wide[3])
                for box in boxes:
                    _, values = query(source.url(box))
                    sizes = (values["size"][0].split(",") if key == "us"
                             else [part.split("(")[1].rstrip(")") for part in values["scalesize"][0].split(",")])
                    self.assertLessEqual(max(int(size) for size in sizes), MAX_PIXELS)


    def test_a_coverage_that_refuses_scalesize_is_asked_without_it(self):
        """Several servers answer `ScaleAxisUndefined` however the axes are named, so those
        rows take the coverage's native step instead."""

        for key in ("de-bw", "uk", "it-bz"):
            with self.subTest(key):
                _, values = query(ingest.SOURCES[key].url(BOX))
                self.assertNotIn("scalesize", values)
                self.assertEqual(len(values["subset"]), 2)
        self.assertIn("scalesize", query(ingest.SOURCES["de-he"].url(BOX))[1])

    def test_the_wcs_11_request_states_the_grid_and_not_an_output_size(self):
        """A 1.1.1 server answers a single pixel when the grid is left out of the request."""

        source = ingest.SOURCES["ca"]
        _, values = query(source.url(BOX))
        self.assertEqual(values["version"], ["1.1.1"])
        self.assertEqual(values["identifier"], ["dtm"])
        self.assertEqual(values["format"], ["image/geotiff"])
        low_x, low_y, high_x, high_y, crs = values["boundingbox"][0].split(",")
        self.assertEqual(crs, f"urn:ogc:def:crs:EPSG::{source.epsg}")
        # The grid starts at the north-west corner and steps east and south from it.
        self.assertEqual(values["gridorigin"], [f"{low_x},{high_y}"])
        step_x, step_y = values["gridoffsets"][0].split(",")
        self.assertEqual((float(step_x), float(step_y)), (1.0, -1.0))
        self.assertGreater(float(high_x), float(low_x))
        self.assertGreater(float(high_y), float(low_y))


BOXES = {
    "de-by": (10.9743, 47.4138, 10.9963, 47.4284),
    "de-sn": (12.9432, 50.4213, 12.9652, 50.4359),
    "de-th": (10.7351, 50.6524, 10.7571, 50.6670),
}


class NamedGrids(unittest.TestCase):
    """The states that publish one file per grid square: the name is arithmetic."""

    def test_a_box_becomes_the_squares_it_touches(self):
        for key, tile_km, expect in (("de-by", 1, "650_5253.tif"),
                                     ("de-sn", 2, "dgm1_33354_5586_2_sn_tiff.zip"),
                                     ("de-th", 1, "dgm1_32_623_5613_1_th_2020-2025.zip")):
            with self.subTest(key):
                source = ingest.SOURCES[key]
                self.assertEqual(source.tile_km, tile_km)
                names = [name for name, _ in source.files(BOXES[key])]
                self.assertIn(expect, names)
                for name, url in source.files(BOXES[key]):
                    self.assertTrue(url.startswith(source.base))
                    self.assertTrue(url.endswith(name))

    def test_a_two_kilometre_grid_only_names_even_squares(self):
        """Saxony publishes 2 km squares, so an odd kilometre is not a file."""

        for name, _ in ingest.SOURCES["de-sn"].files(BOXES["de-sn"]):
            parts = name.split("_")
            east, north = int(parts[1][2:]), int(parts[2])
            self.assertEqual((east % 2, north % 2), (0, 0), name)

    def test_the_squares_of_a_box_are_on_the_grid_and_distinct(self):
        squares = list(grid_squares((10.97, 47.41, 11.00, 47.43), 25832, 1000))
        self.assertTrue(all(x % 1000 == 0 and y % 1000 == 0 for x, y in squares))
        self.assertEqual(len(squares), len(set(squares)))


class StacSearch(unittest.TestCase):
    """Lower Saxony's tiles are indexed only by a search, so the search is the adapter."""

    def setUp(self):
        self.source = ingest.SOURCES["de-ni"]
        real = ingest.sources.stac.http_get
        self.addCleanup(lambda: setattr(ingest.sources.stac, "http_get", real))
        self.seen = []

    def answer(self, body):
        def get(url):
            self.seen.append(url)
            return body
        ingest.sources.stac.http_get = get

    def test_the_named_asset_of_every_item_becomes_a_file(self):
        self.answer(b'{"features": ['
                    b'{"id": "a", "assets": {"dgm1-tif": {"href": "https://x/L25/a.tif"}}},'
                    b'{"id": "b", "assets": {"dgm1-tif": {"href": "https://x/L16/b.tif"}}}]}')
        self.assertEqual(self.source.files((10.6, 51.75, 10.63, 51.77)),
                         [("a.tif", "https://x/L25/a.tif"), ("b.tif", "https://x/L16/b.tif")])
        self.assertIn("bbox=10.6,51.75,10.63,51.77", self.seen[0])

    def test_an_item_without_the_asset_is_refused_by_name(self):
        self.answer(b'{"features": [{"id": "a", "assets": {"thumbnail": {"href": "x"}}}]}')
        with self.assertRaises(ingest.Refuse) as refusal:
            self.source.files((10.6, 51.75, 10.63, 51.77))
        self.assertIn("dgm1-tif", str(refusal.exception))

    def test_an_answer_that_is_not_a_stac_search_is_refused(self):
        self.answer(b'{"message": "gateway timeout"}')
        with self.assertRaises(ingest.Refuse) as refusal:
            self.source.files((10.6, 51.75, 10.63, 51.77))
        self.assertIn("gateway timeout", str(refusal.exception))


class RemoteWindows(TempCase):
    """Austria's squares are 6.5 GB each, so the box's window is read and nothing else."""

    BOX = (12.68, 47.067, 12.6827, 47.0687)

    def remote(self, directory):
        return ingest.sources.cog.CogGrid(
            "xx", "Nowhere", "DTM 1 m", 1.0, "CC BY 4.0", "(c) Nowhere", "DHHN2016",
            (-180, -90, 180, 90),
            base=f"{directory}/", name="tile_{north}_{east}.tif", epsg=3035, tile_m=50000)

    def test_only_the_window_the_box_needs_comes_out_of_a_square(self):
        """The window carries a pixel of margin and keeps the square's own transform."""

        square = self.root / "remote"
        square.mkdir()
        source = self.remote(square)
        # One square of the grid the adapter names, holding a 4 km patch around the box.
        (name, _), = source.urls(self.BOX)
        (lo_x, lo_y, hi_x, hi_y), _ = ingest.sources.protocols.projected_box(self.BOX, 3035)
        origin_x, origin_y = round(lo_x) - 2000, round(hi_y) + 2000
        heights = (np.arange(4000 * 4000, dtype="float32") % 3000).reshape(4000, 4000)
        with rasterio.open(square / name, "w", driver="GTiff",
                           width=4000, height=4000, count=1, dtype="float32",
                           crs=CRS.from_epsg(3035), nodata=-9999.0,
                           transform=Affine(1, 0, origin_x, 0, -1, origin_y)) as dst:
            dst.write(heights, 1)

        cut = source.fetch(self.BOX, self.root / "work")
        self.assertEqual(len(cut), 1)
        with rasterio.open(cut[0]) as src:
            self.assertEqual(src.crs, CRS.from_epsg(3035))
            self.assertLess(src.width * src.height, 4000 * 4000)
            self.assertEqual(src.res, (1.0, 1.0))
            # The margin means the window reaches past the box on every side.
            self.assertLessEqual(src.bounds.left, lo_x)
            self.assertGreaterEqual(src.bounds.right, hi_x)
            self.assertLessEqual(src.bounds.bottom, lo_y)
            self.assertGreaterEqual(src.bounds.top, hi_y)

        # The cut is cached, so a second run reads nothing remote.
        (square / name).unlink()
        self.assertEqual(source.fetch(self.BOX, self.root / "work"), cut)

    def test_a_square_that_is_not_published_is_named_in_the_refusal(self):
        empty = self.root / "empty"
        empty.mkdir()
        with self.assertRaises(ingest.Refuse) as refusal:
            self.remote(empty).fetch(self.BOX, self.root / "work2")
        self.assertIn("is published", str(refusal.exception))


class StampedCrs(TempCase):
    def test_an_answer_with_no_crs_takes_the_grid_the_row_declares(self):
        """A coverage that answers a GeoTIFF with a transform and no projection at all
        cannot be placed by the tail, and the row already names the grid it asked in."""

        source = ingest.SOURCES["de-mv"]
        path = self.inputs / "nameless.tif"
        with rasterio.open(path, "w", driver="GTiff", width=4, height=4, count=1,
                           dtype="float32",
                           transform=Affine(1, 0, 300000, 0, -1, 5950300)) as dst:
            dst.write(np.full((4, 4), 100.0, dtype="float32"), 1)
        with rasterio.open(path) as src:
            self.assertIsNone(src.crs)

        source.stamp_crs(path)
        with rasterio.open(path) as src:
            self.assertEqual(src.crs, CRS.from_epsg(source.epsg))


class FrenchBil(unittest.TestCase):
    """France is served as the band alone, so the adapter builds the GeoTIFF around it."""

    def setUp(self):
        self.source = ingest.SOURCES["fr"]
        self.px, self.py = 1858, 1781
        real = fr_module.http_get
        self.addCleanup(lambda: setattr(fr_module, "http_get", real))

    def answer(self, body):
        fr_module.http_get = lambda url: body

    def test_the_raw_float32_band_becomes_a_placed_geotiff(self):
        """The bytes carry no header, so the size and the box the request stated place them."""

        # A plausible alpine range, so the only void in the raster is the sentinel.
        ramp = np.arange(self.py * self.px, dtype="<f4") % 250_000 / 100.0
        heights = ramp.reshape(self.py, self.px).astype("<f4")
        heights[7, 11] = fr_module.IGN_VOID
        self.answer(heights.tobytes())

        with TemporaryDirectory() as directory:
            path = self.source.fetch(BOX, Path(directory))[0]
            with rasterio.open(path) as src:
                self.assertEqual((src.width, src.height), (self.px, self.py))
                self.assertEqual(src.crs, ingest.WGS84)
                self.assertEqual(src.nodata, fr_module.IGN_VOID)
                west, south, east, north = src.bounds
                self.assertAlmostEqual(west, BOX[0], places=9)
                self.assertAlmostEqual(north, BOX[3], places=9)
                self.assertAlmostEqual(east, BOX[2], places=9)
                self.assertAlmostEqual(south, BOX[1], places=9)
                # Row 0 of the answer is the north edge, which is where a GeoTIFF wants it.
                self.assertEqual(src.read(1)[0, 0], heights[0, 0])
                self.assertEqual(src.read(1)[7, 11], fr_module.IGN_VOID)

            # And the tail turns that sentinel into absence, like any other void.
            values, _, _, _, voided = ingest.read_source(path)
            self.assertEqual(values[7, 11], ingest.VOID)
            self.assertAlmostEqual(voided, 1 / (self.px * self.py), places=9)

    def test_an_answer_that_is_not_the_band_is_refused_with_what_came_back(self):
        """A WMS answers an error as XML with a 200, so the length is the only tell."""

        self.answer(b'<?xml version="1.0"?><ServiceExceptionReport>'
                    b'<ServiceException code="LayerNotDefined">gone</ServiceException>')
        with TemporaryDirectory() as directory:
            with self.assertRaises(ingest.Refuse) as refusal:
                self.source.fetch(BOX, Path(directory))
        message = str(refusal.exception)
        self.assertIn(f"{self.px}x{self.py} float32", message)
        self.assertIn("LayerNotDefined", message)

    def test_the_request_asks_for_bil_with_the_box_latitude_first(self):
        """WMS 1.3.0 on a geographic CRS states the box latitude first, and a swapped box
        answers with terrain from somewhere else entirely."""

        _, values = query(self.source.url(BOX))
        self.assertEqual(values["format"], ["image/x-bil;bits=32"])
        self.assertEqual(values["crs"], ["EPSG:4326"])
        self.assertEqual(values["bbox"], [f"{BOX[1]},{BOX[0]},{BOX[3]},{BOX[2]}"])
        self.assertEqual((values["width"], values["height"]), ([str(self.px)], [str(self.py)]))


class BulkArchives(unittest.TestCase):
    """A bulk product arrives as a zip of tiles, and only the rasters come out of it."""

    def bundle(self, directory, members):
        path = Path(directory) / "state.zip"
        with zipfile.ZipFile(path, "w") as bundle:
            for name, body in members.items():
                bundle.writestr(name, body)
        return path

    def test_only_the_rasters_are_taken_out_and_the_rest_is_left(self):
        with TemporaryDirectory() as directory:
            archive = self.bundle(directory, {
                "dgm/tile_01.tif": b"II*\x00 not really a tiff",
                "dgm/tile_01.asc": b"ncols 2",
                "readme.txt": b"licence",
                "dgm/thumb.png": b"\x89PNG",
            })
            out = Path(directory) / "unpacked"
            rasters = ingest.sources.base.unpack(archive, out)
            self.assertEqual([path.relative_to(out).as_posix() for path in rasters],
                             ["dgm/tile_01.asc", "dgm/tile_01.tif"])
            self.assertFalse((out / "readme.txt").exists())

    def test_a_member_that_reaches_outside_the_directory_is_refused(self):
        """A zip can name `../../etc/x.tif`, and unpacking it would write there."""

        with TemporaryDirectory() as directory:
            archive = self.bundle(directory, {"../../escaped.tif": b"II*\x00"})
            with self.assertRaises(ingest.Refuse) as refusal:
                ingest.sources.base.unpack(archive, Path(directory) / "unpacked")
            self.assertIn("reaches outside", str(refusal.exception))

    def test_a_bulk_source_downloads_unpacks_and_caches(self):
        """`files` is the whole adapter; the download, the unpack and the cache are shared."""

        class Fake(ingest.sources.bulk.BulkSource):
            def files(self, bbox):
                return [("state.zip", f"file://{self.served}")]

        with TemporaryDirectory() as directory:
            served = self.bundle(directory, {"dgm/tile_01.tif": b"II*\x00", "readme.txt": b"x"})
            source = Fake("xx", "Nowhere", "DGM 1 m", 1.0, "CC BY 4.0", "© Nowhere",
                          "DHHN2016", (-180, -90, 180, 90))
            source.served = served
            work = Path(directory) / "work"
            rasters = source.fetch((0, 0, 1, 1), work)
            self.assertEqual([path.name for path in rasters], ["tile_01.tif"])

            # The cache: the second run reads the downloaded zip and fetches nothing.
            served.unlink()
            self.assertEqual(source.fetch((0, 0, 1, 1), work), rasters)

    def test_a_bulk_source_that_covers_nothing_says_so(self):
        class Empty(ingest.sources.bulk.BulkSource):
            def files(self, bbox):
                return []

        source = Empty("xx", "Nowhere", "p", 1.0, "l", "a", "d", (-180, -90, 180, 90))
        with self.assertRaises(ingest.Refuse) as refusal:
            source.fetch((0, 0, 1, 1), Path("/nonexistent"))
        self.assertIn("nothing published covers", str(refusal.exception))

    def test_an_archive_with_no_raster_says_so(self):
        with TemporaryDirectory() as directory:
            archive = self.bundle(directory, {"readme.txt": b"licence only"})
            with self.assertRaises(ingest.Refuse):
                ingest.sources.base.unpack(archive, Path(directory) / "unpacked")


class Registry(unittest.TestCase):
    def test_a_service_that_answers_xml_instead_of_a_raster_is_refused_by_name(self):
        """Out of coverage, renamed, or down: every one of them arrives as a 200 and XML."""

        real = ingest.sources.protocols.http_get
        self.addCleanup(lambda: setattr(ingest.sources.protocols, "http_get", real))
        ingest.sources.protocols.http_get = lambda url: b"<ExceptionReport>no such coverage</ExceptionReport>"
        with self.assertRaises(ingest.Refuse) as refusal:
            ingest.SOURCES["de-nw"].request(BOX)
        self.assertIn("no such coverage", str(refusal.exception))

    def test_every_registered_source_states_a_vertical_datum(self):
        """The archive is orthometric metres, so a source whose datum nobody wrote down
        cannot be held to it."""

        for key, source in ingest.SOURCES.items():
            with self.subTest(key):
                self.assertTrue(source.vertical_datum)
                self.assertTrue(source.attribution and source.licence and source.product)
        with self.assertRaises(ingest.Refuse):
            ingest.Source("x", "Nowhere", "p", 1.0, "l", "a", "", (0, 0, 1, 1))

    def test_a_source_with_no_adapter_says_to_pass_input(self):
        manual = ingest.sources.ManualSource(
            "xx", "Nowhere", "p", 1.0, "l", "a", "d", (0, 0, 1, 1),
            why="the service needs a token")
        with self.assertRaises(ingest.Refuse) as refusal:
            manual.fetch(BOX, Path("/nonexistent"))
        self.assertIn("the service needs a token", str(refusal.exception))
        self.assertIn("--input", str(refusal.exception))


if __name__ == "__main__":
    unittest.main()
