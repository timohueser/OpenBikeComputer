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
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from tempfile import TemporaryDirectory
from threading import Thread

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
from ingest.sources.nz import sheets as nz_sheets  # noqa: E402
from ingest.sources.protocols import MAX_PIXELS, request_boxes  # noqa: E402

BOX = (6.8255, 45.9257, 6.8495, 45.9417)  # about 1.9 x 1.8 km over Chamonix

#: The verification box of every source with an adapter, exactly as `README.md` documents
#: it. A request is only ever built from a box the split has already cut, so these tests
#: go through `request_boxes` the way `TiledService.fetch` does.
BOXES = {
    "ch": (8.3800, 46.7800, 8.4200, 46.8200),
    "fr": (2.8019, 45.5204, 2.8259, 45.5364),
    "us": (-106.4583, 39.1091, -106.4323, 39.1265),
    "no": (8.2925, 61.6230, 8.3325, 61.6496),
    "es": (-4.8666, 43.1888, -4.8406, 43.2062),
    "nl": (6.0079, 50.7453, 6.0339, 50.7627),
    "uk": (-3.2227, 54.4469, -3.2007, 54.4615),
    "at": (12.6829, 47.0671, 12.7049, 47.0817),
    "ca": (-73.5983, 45.4968, -73.5763, 45.5114),
    "it-bz": (10.5337, 46.5016, 10.5557, 46.5162),
    "de-nw": (8.5462, 51.2682, 8.5722, 51.2856),
    "de-he": (9.9287, 50.4908, 9.9507, 50.5054),
    "de-bw": (7.9934, 47.8666, 8.0154, 47.8812),
    "de-mv": (13.5984, 53.4794, 13.6204, 53.4941),
    "de-st": (10.6046, 51.7918, 10.6266, 51.8064),
    "de-by": (10.9743, 47.4138, 10.9963, 47.4284),
    "de-sn": (12.9432, 50.4213, 12.9652, 50.4359),
    "de-th": (10.7351, 50.6524, 10.7571, 50.6670),
    "de-ni": (10.6084, 51.7508, 10.6304, 51.7654),
    "dk": (9.8220, 56.2948, 9.8440, 56.3094),
    "se": (18.4830, 67.8955, 18.5050, 67.9101),
    "fi": (21.2576, 69.2995, 21.2796, 69.3141),
    "nz": (170.1310, -43.6020, 170.1530, -43.5880),
    "it-tn": (10.8620, 46.1510, 10.8840, 46.1656),
}


def split(key):
    """The sub-boxes `fetch` would ask for, for one source's verification box."""

    source = ingest.SOURCES[key]
    return list(request_boxes(BOXES[key], source.resolution_m, getattr(source, "epsg", None)))


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
    """Every live adapter's request, held as the URL it would send.

    The box is the source's own verification box, cut by `request_boxes` first, because a
    request is never built from anything else and `output_size` refuses a box the split
    left too large.
    """

    def test_every_split_sub_box_stays_inside_the_pixel_cap(self):
        """The split is what keeps a request from coming back coarsened, so it is the
        first thing to hold: every adapter, every sub-box of its documented box."""

        for key in BOXES:
            source = ingest.SOURCES[key]
            if not isinstance(source, ingest.sources.protocols.TiledService):
                continue  # a tile grid and a STAC search are not sized in pixels
            with self.subTest(key):
                boxes = split(key)
                self.assertGreaterEqual(len(boxes), 1)
                for box in boxes:
                    span_x, span_y = ingest.sources.protocols.spans(box, source.epsg)
                    self.assertLessEqual(span_x / source.resolution_m, MAX_PIXELS + 1)
                    self.assertLessEqual(span_y / source.resolution_m, MAX_PIXELS + 1)
                # The sub-boxes tile the box exactly.
                self.assertAlmostEqual(min(b[0] for b in boxes), BOXES[key][0])
                self.assertAlmostEqual(min(b[1] for b in boxes), BOXES[key][1])
                self.assertAlmostEqual(max(b[2] for b in boxes), BOXES[key][2])
                self.assertAlmostEqual(max(b[3] for b in boxes), BOXES[key][3])

    def test_a_request_over_the_cap_is_refused_and_not_coarsened(self):
        """Asking a service for fewer pixels than the box holds gets a coarser raster
        back, and a quietly coarser reference is the failure this archive must not have."""

        with self.assertRaises(ingest.Refuse) as refusal:
            ingest.sources.protocols.pixels(MAX_PIXELS + 1, "a box nobody split")
        self.assertIn("was not split small enough", str(refusal.exception))

    def test_the_arcgis_request_asks_for_float32_metres_in_wgs84(self):
        """3DEP resamples to the size it is asked for, so the size and the type are stated."""

        box = split("us")[0]
        path, values = query(ingest.SOURCES["us"].url(box))
        self.assertTrue(path.endswith("/3DEPElevation/ImageServer/exportImage"))
        self.assertEqual(values["bbox"], [",".join(str(value) for value in box)])
        self.assertEqual((values["bboxsr"], values["imagesr"]), (["4326"], ["4326"]))
        self.assertEqual((values["format"], values["pixeltype"], values["f"]),
                         (["tiff"], ["F32"], ["image"]))
        px, py = (int(value) for value in values["size"][0].split(","))
        self.assertLessEqual(max(px, py), MAX_PIXELS)
        self.assertGreater(min(px, py), 1000)

    def test_a_wcs_20_request_names_the_coverage_its_axes_and_the_output_size(self):
        """WCS 2.0 answers at the coverage's native step unless it is asked otherwise, and
        a label the server does not know is a refusal, so both are in every request."""

        for key, coverage, epsg, axes in (("es", "Elevacion4258_5", 4326, ("long", "lat")),
                                          ("nl", "dtm_05m", 28992, ("x", "y")),
                                          ("de-nw", "nw_dgm", 25832, ("x", "y")),
                                          ("de-he", "he_dgm1", 25832, ("E", "N"))):
            with self.subTest(key):
                source = ingest.SOURCES[key]
                _, values = query(source.url(split(key)[0]))
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

        A projected grid's axes are not the box's and its edges are curves, so the request
        is the rectangle that contains the densified edges. It reaches a little past the
        box, which is what keeps a curved edge from cutting a corner off.
        """

        source = ingest.SOURCES["de-nw"]
        box = split("de-nw")[0]
        _, values = query(source.url(box))
        subsets = {part.split("(")[0]: part.split("(")[1].rstrip(")").split(",")
                   for part in values["subset"]}
        to_grid = Transformer.from_crs("EPSG:4326", f"EPSG:{source.epsg}", always_xy=True)
        for lon, lat in ((box[0], box[1]), (box[2], box[1]), (box[0], box[3]), (box[2], box[3])):
            x, y = to_grid.transform(lon, lat)
            self.assertLessEqual(float(subsets["x"][0]), x)
            self.assertGreaterEqual(float(subsets["x"][1]), x)
            self.assertLessEqual(float(subsets["y"][0]), y)
            self.assertGreaterEqual(float(subsets["y"][1]), y)

    def test_the_wcs_10_request_states_a_bbox_with_a_width_and_a_height(self):
        """Norway and Denmark answer WCS 1.0.0, which sizes its grid differently from
        2.0.1 and names a format with the server's own word rather than a media type."""

        for key, coverage, epsg, image in (("no", "nhm_dtm_topo_25833", 25833, "GeoTIFF"),
                                           ("dk", "dhm_terraen", 25832, "GTiff")):
            with self.subTest(key):
                source = ingest.SOURCES[key]
                _, values = query(source.url(split(key)[0]))
                self.assertEqual(values["version"], ["1.0.0"])
                self.assertEqual(values["coverage"], [coverage])
                self.assertEqual(values["crs"], [f"EPSG:{epsg}"])
                self.assertEqual(values["format"], [image])
                self.assertNotIn("subset", values)
                self.assertEqual(len(values["bbox"][0].split(",")), 4)
                self.assertLessEqual(max(int(values["width"][0]), int(values["height"][0])),
                                     MAX_PIXELS)

    def test_a_coverage_that_refuses_scalesize_is_asked_without_it(self):
        """Several servers answer `ScaleAxisUndefined` however the axes are named, so those
        rows take the coverage's native step instead."""

        for key in ("de-bw", "uk", "it-bz"):
            with self.subTest(key):
                _, values = query(ingest.SOURCES[key].url(split(key)[0]))
                self.assertNotIn("scalesize", values)
                self.assertEqual(len(values["subset"]), 2)
        self.assertIn("scalesize", query(ingest.SOURCES["de-he"].url(split("de-he")[0]))[1])

    def test_the_wcs_11_request_states_the_grid_at_the_products_own_step(self):
        """A 1.1.1 server answers a single pixel when the grid is left out of the request,
        and a coarser step if it is asked for one, so the step is always the product's."""

        source = ingest.SOURCES["ca"]
        _, values = query(source.url(split("ca")[0]))
        self.assertEqual(values["version"], ["1.1.1"])
        self.assertEqual(values["identifier"], ["dtm"])
        self.assertEqual(values["format"], ["image/geotiff"])
        low_x, low_y, high_x, high_y, crs = values["boundingbox"][0].split(",")
        self.assertEqual(crs, f"urn:ogc:def:crs:EPSG::{source.epsg}")
        # The grid starts at the north-west corner and steps east and south from it.
        self.assertEqual(values["gridorigin"], [f"{low_x},{high_y}"])
        self.assertEqual(values["gridoffsets"], ["1.0,-1.0"])
        self.assertGreater(float(high_x), float(low_x))
        self.assertGreater(float(high_y), float(low_y))

    def test_a_box_larger_than_the_cap_becomes_several_requests(self):
        """Half-metre LiDAR overruns every server's size cap, so the box is cut up."""

        wide = (6.0, 46.0, 6.2, 46.2)  # about 15 km by 22 km
        for key in ("us", "nl"):
            with self.subTest(key):
                source = ingest.SOURCES[key]
                boxes = list(request_boxes(wide, source.resolution_m, source.epsg))
                self.assertGreater(len(boxes), 1)
                self.assertAlmostEqual(min(box[0] for box in boxes), wide[0])
                self.assertAlmostEqual(max(box[3] for box in boxes), wide[3])
                for box in boxes:
                    _, values = query(source.url(box))
                    sizes = (values["size"][0].split(",") if key == "us"
                             else [part.split("(")[1].rstrip(")")
                                   for part in values["scalesize"][0].split(",")])
                    self.assertLessEqual(max(int(size) for size in sizes), MAX_PIXELS)


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

    def test_the_nztopo50_sheet_of_a_box_is_the_one_linz_publishes(self):
        """New Zealand's index is arithmetic, so these constants are the whole adapter.

        The corners are read off the published tiles: sheet `AS21` runs from NZTM2000
        (1 492 000, 6 198 000), and `BX15`, the sheet Aoraki stands on, from
        (1 348 000, 5 154 000). A wrong constant names a sheet whose window misses the
        box, which refuses rather than writing the wrong heights, and this holds it
        before that.
        """

        to_wgs84 = Transformer.from_crs(2193, 4326, always_xy=True)
        for sheet, (east, north) in (("AS21", (1_492_000, 6_198_000)),
                                     ("AS22", (1_516_000, 6_198_000)),
                                     ("AT21", (1_492_000, 6_162_000)),
                                     ("BX15", (1_348_000, 5_154_000)),
                                     ("CJ21", (1_492_000, 4_758_000))):
            with self.subTest(sheet):
                # A small box well inside the sheet, so only that one sheet is named.
                lon, lat = to_wgs84.transform(east + 12_000, north + 18_000)
                self.assertEqual(nz_sheets((lon - 0.002, lat - 0.002, lon + 0.002, lat + 0.002)),
                                 [sheet])

        # Aoraki's own box, which the live probe was measured over.
        source = ingest.SOURCES["nz"]
        self.assertEqual([name for name, _ in source.urls(BOXES["nz"])], ["BX15.tiff"])
        self.assertTrue(source.urls(BOXES["nz"])[0][1].endswith(
            "/new-zealand/new-zealand/dem_1m/2193/BX15.tiff"))

    def test_the_trentino_grid_square_is_named_after_its_corner(self):
        """Trentino's file name is the square's corner in hundreds of metres.

        That is what the province's own WFS index says: the square at EPSG:25832
        (643 500, 5 112 000) is `5h643551120_DTM.asc`. So no index has to be read.
        """

        source = ingest.SOURCES["it-tn"]
        to_wgs84 = Transformer.from_crs(25832, 4326, always_xy=True)
        lon, lat = to_wgs84.transform(643_500 + 250, 5_112_000 + 250)
        inside = (lon - 0.0005, lat - 0.0005, lon + 0.0005, lat + 0.0005)
        self.assertEqual([name for name, _ in source.files(inside)],
                         ["5h643551120_DTM.asc"])

        names = [name for name, _ in source.files(BOXES["it-tn"])]
        self.assertEqual(sorted(names), names)
        self.assertEqual(len(names), len(set(names)))
        for name, url in source.files(BOXES["it-tn"]):
            self.assertRegex(name, r"^5h\d{4}\d{5}_DTM\.asc$")
            self.assertEqual(url,
                             "https://siatservices.provincia.tn.it/stemdata/"
                             f"2014_lidar_dtm_asc/{name}")
        # A square the survey did not reach answers 404, which is a coverage edge.
        self.assertTrue(source.skip_missing)

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


class RangeHandler(SimpleHTTPRequestHandler):
    """A static file server that answers byte ranges, which is what a COG reader needs."""

    def log_message(self, *args):
        pass

    def do_GET(self):  # noqa: N802
        path = Path(self.translate_path(self.path))
        if not path.is_file():
            self.send_error(404, "no such square")
            return
        body = path.read_bytes()
        asked = self.headers.get("Range")
        if not asked:
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Accept-Ranges", "bytes")
            self.end_headers()
            self.wfile.write(body)
            return
        first, _, last = asked.removeprefix("bytes=").partition("-")
        start = int(first)
        stop = int(last) if last else len(body) - 1
        chunk = body[start:stop + 1]
        self.send_response(206)
        self.send_header("Content-Range", f"bytes {start}-{start + len(chunk) - 1}/{len(body)}")
        self.send_header("Content-Length", str(len(chunk)))
        self.send_header("Accept-Ranges", "bytes")
        self.end_headers()
        self.wfile.write(chunk)


class RemoteWindows(TempCase):
    """Austria's squares are 7.7 GB each, so the box's window is read and nothing else.

    The squares are served over real HTTP from a thread, because the two things worth
    holding here are what the adapter does with a 404 and what it caches a cut under, and
    both of those are HTTP behaviour.
    """

    BOX = (12.68, 47.067, 12.6827, 47.0687)
    #: A second box a few hundred metres away, in the same 50 km square. A cache keyed on
    #: the square alone would hand this one the first box's heights, which is the bug this
    #: class exists for.
    OTHER = (12.6850, 47.0700, 12.6877, 47.0717)

    def setUp(self):
        super().setUp()
        self.served = self.root / "served"
        self.served.mkdir()
        handler = partial(RangeHandler, directory=str(self.served))
        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), handler)
        Thread(target=self.httpd.serve_forever, daemon=True).start()
        self.addCleanup(self.httpd.server_close)
        self.addCleanup(self.httpd.shutdown)
        self.source = ingest.sources.cog.CogGrid(
            "xx", "Nowhere", "DTM 1 m", 1.0, "CC BY 4.0", "(c) Nowhere", "DHHN2016",
            (-180, -90, 180, 90),
            base=f"http://127.0.0.1:{self.httpd.server_address[1]}/",
            name="tile_{north}_{east}.tif", epsg=3035, tile_m=50000)

    def serve_square(self, *boxes):
        """The one grid square those boxes share, holding a patch that covers them all."""

        names = {name for box in boxes for name, _ in self.source.urls(box)}
        self.assertEqual(len(names), 1, f"the boxes must share one square, not {names}")
        name = names.pop()
        corners = [ingest.sources.protocols.projected_box(box, 3035)[0] for box in boxes]
        lo_x = min(corner[0] for corner in corners) - 200
        lo_y = min(corner[1] for corner in corners) - 200
        hi_x = max(corner[2] for corner in corners) + 200
        hi_y = max(corner[3] for corner in corners) + 200
        width, height = round(hi_x - lo_x), round(hi_y - lo_y)
        heights = (np.arange(width * height, dtype="float32") % 3000).reshape(height, width)
        with rasterio.open(self.served / name, "w", driver="GTiff", width=width, height=height,
                           count=1, dtype="float32", crs=CRS.from_epsg(3035), nodata=-9999.0,
                           transform=Affine(1, 0, round(lo_x), 0, -1, round(hi_y))) as dst:
            dst.write(heights, 1)
        return name, (width, height)

    def test_only_the_window_the_box_needs_comes_out_of_a_square(self):
        """The window carries a pixel of margin and keeps the square's own transform."""

        name, (width, height) = self.serve_square(self.BOX)
        (lo_x, lo_y, hi_x, hi_y), _ = ingest.sources.protocols.projected_box(self.BOX, 3035)
        cut = self.source.fetch(self.BOX, self.root / "work")
        self.assertEqual(len(cut), 1)
        with rasterio.open(cut[0]) as src:
            self.assertEqual(src.crs, CRS.from_epsg(3035))
            self.assertLess(src.width * src.height, width * height)
            self.assertEqual(src.res, (1.0, 1.0))
            # The margin means the window reaches past the box on every side.
            self.assertLessEqual(src.bounds.left, lo_x)
            self.assertGreaterEqual(src.bounds.right, hi_x)
            self.assertLessEqual(src.bounds.bottom, lo_y)
            self.assertGreaterEqual(src.bounds.top, hi_y)

        # The cut is cached, so a second run of the same box asks the server nothing: the
        # square is gone and the server is stopped, and the cut still comes back.
        (self.served / name).unlink()
        self.httpd.shutdown()
        self.assertEqual(self.source.fetch(self.BOX, self.root / "work"), cut)

    def test_a_second_box_in_one_square_gets_its_own_heights(self):
        """The cut is cached under the square *and* the box. Keyed on the square alone,
        the second box in a 50 km square silently reads the first box's heights."""

        self.serve_square(self.BOX, self.OTHER)
        work = self.root / "work"
        here = self.source.fetch(self.BOX, work)
        there = self.source.fetch(self.OTHER, work)
        self.assertNotEqual(here, there)
        with rasterio.open(here[0]) as one, rasterio.open(there[0]) as two:
            self.assertNotEqual(one.bounds, two.bounds)
            for bounds, box in ((one.bounds, self.BOX), (two.bounds, self.OTHER)):
                (lo_x, lo_y, hi_x, hi_y), _ = ingest.sources.protocols.projected_box(box, 3035)
                self.assertLessEqual(bounds.left, lo_x)
                self.assertGreaterEqual(bounds.right, hi_x)

    def test_a_square_that_is_not_published_is_a_coverage_edge(self):
        """404 is the country's edge. The box is then covered by nothing, which is said."""

        with self.assertRaises(ingest.Refuse) as refusal:
            self.source.fetch(self.BOX, self.root / "work")
        self.assertIn("no published square", str(refusal.exception))

    def test_a_server_fault_is_not_mistaken_for_a_coverage_edge(self):
        """A 500 or a reset must not become a silent hole in the archive."""

        class Broken(SimpleHTTPRequestHandler):
            def do_GET(self):  # noqa: N802
                self.send_error(500, "upstream is down")

            def log_message(self, *args):
                pass

        broken = ThreadingHTTPServer(("127.0.0.1", 0), Broken)
        Thread(target=broken.serve_forever, daemon=True).start()
        self.addCleanup(broken.server_close)
        self.addCleanup(broken.shutdown)
        self.source.base = f"http://127.0.0.1:{broken.server_address[1]}/"

        ingest.sources.base.RETRY_DELAYS = ()
        self.addCleanup(lambda: setattr(ingest.sources.base, "RETRY_DELAYS", (2, 4, 8)))
        with self.assertRaises(ingest.Refuse) as refusal:
            self.source.fetch(self.BOX, self.root / "work")
        self.assertIn("500", str(refusal.exception))


class FrenchBil(unittest.TestCase):
    """France is served as the band alone, so the adapter builds the GeoTIFF around it."""

    def setUp(self):
        self.source = ingest.SOURCES["fr"]
        self.px, self.py = 1858, 1781
        real = fr_module.http_get
        self.addCleanup(lambda: setattr(fr_module, "http_get", real))

    def answer(self, body):
        fr_module.http_get = lambda url, what=None: body

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
                             ["dgm/tile_01.tif"])
            self.assertFalse((out / "readme.txt").exists())
            # A `.asc` grid names no CRS, and the tail cannot place one, so it stays in.
            self.assertFalse((out / "dgm" / "tile_01.asc").exists())

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

        source = Empty("xx", "Nowhere", "p", 1.0, "l", "a", "NAP", (-180, -90, 180, 90))
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
        ingest.sources.protocols.http_get = (
            lambda url, what=None: b"<ExceptionReport>no such coverage</ExceptionReport>")
        with self.assertRaises(ingest.Refuse) as refusal:
            ingest.SOURCES["de-nw"].request(BOX)
        self.assertIn("no such coverage", str(refusal.exception))

    def test_every_registered_source_is_ranked(self):
        """A key that is not in `PRIORITY` ranks last, which is a silent demotion."""

        self.assertEqual([key for key in ingest.SOURCES if key not in ingest.PRIORITY], [])

    def test_every_registered_source_states_a_vertical_datum(self):
        """The archive is orthometric metres, so a source whose datum nobody wrote down
        cannot be held to it."""

        for key, source in ingest.SOURCES.items():
            with self.subTest(key):
                self.assertTrue(source.vertical_datum)
                self.assertTrue(source.attribution and source.licence and source.product)
        for datum in ("", "NAD83 ellipsoidal heights", "Mystery Height 1997"):
            with self.subTest(datum):
                with self.assertRaises(ingest.Refuse):
                    ingest.Source("x", "Nowhere", "p", 1.0, "l", "a", datum, (0, 0, 1, 1))

    def test_a_source_with_no_adapter_says_to_pass_input(self):
        manual = ingest.sources.ManualSource(
            "xx", "Nowhere", "p", 1.0, "l", "a", "NAP", (0, 0, 1, 1),
            why="the service needs a token")
        with self.assertRaises(ingest.Refuse) as refusal:
            manual.fetch(BOX, Path("/nonexistent"))
        self.assertIn("the service needs a token", str(refusal.exception))
        self.assertIn("--input", str(refusal.exception))


if __name__ == "__main__":
    unittest.main()
