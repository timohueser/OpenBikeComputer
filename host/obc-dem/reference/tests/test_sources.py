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

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import ingest  # noqa: E402
import ingest.sources.base  # noqa: E402
import ingest.sources.bulk  # noqa: E402
from ingest.sources import fr as fr_module  # noqa: E402
from ingest.sources.protocols import MAX_PIXELS, request_boxes  # noqa: E402

BOX = (6.8255, 45.9257, 6.8495, 45.9417)  # about 1.9 x 1.8 km over Chamonix


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

    def test_a_projected_wcs_request_states_the_box_in_the_services_own_grid(self):
        """The subsets are metres on the national grid, not degrees."""

        source = ingest.SOURCES["de-nw"]
        _, values = query(source.url(BOX))
        subsets = {part.split("(")[0]: part.split("(")[1].rstrip(")").split(",")
                   for part in values["subset"]}
        low = (float(subsets["x"][0]), float(subsets["y"][0]))
        back = Transformer.from_crs(f"EPSG:{source.epsg}", "EPSG:4326", always_xy=True).transform(*low)
        self.assertAlmostEqual(back[0], BOX[0], places=6)
        self.assertAlmostEqual(back[1], BOX[1], places=6)

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
