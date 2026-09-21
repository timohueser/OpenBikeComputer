"""A source behind an account: the delivered files, the refusal, and the wizard.

No test here reaches the network or wants a credential. What the portals deliver is built
synthetically, in the format `README.md` documents for each one — the name pattern, the
CRS, the dtype and the nodata value — and put through `ingest --input`, because that path
is the whole adapter for a source nobody can fetch unattended. The live probes are in
`README.md`.
"""

import argparse
import io
import os
import sys
import unittest
import urllib.error
import urllib.parse
import zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
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

#: What each portal delivers, as `README.md` states it. `corner` is the south-west corner
#: of the synthetic tile on the source's own grid, inside the country the row covers.
DELIVERIES = {
    "dk": {"name": "DTM_1km_6171_723.tif", "epsg": 25832, "corner": (723_000, 6_171_000),
           "step": 0.4, "dtype": "float32", "nodata": -9999.0},
    "se": {"name": "m753_64.tif", "epsg": 5845, "corner": (640_000, 7_530_000),
           "step": 1.0, "dtype": "float32", "nodata": -9999.0},
    "fi": {"name": "N4331A.tif", "epsg": 3067, "corner": (370_000, 6_700_000),
           "step": 2.0, "dtype": "float32", "nodata": -9999.0},
    "nz": {"name": "BX15.tiff", "epsg": 2193, "corner": (1_360_000, 5_170_000),
           "step": 1.0, "dtype": "float32", "nodata": -9999.0},
    "au": {"name": "Wollongong_2020_DEM.tif", "epsg": 28356, "corner": (300_000, 6_180_000),
           "step": 1.0, "dtype": "float32", "nodata": -9999.0, "zip": "elvis_order.zip"},
    "it-tn": {"name": "5h643551120_DTM.asc", "epsg": 25832, "corner": (643_500, 5_112_000),
              "step": 0.5, "dtype": "float32", "nodata": -9999.0, "grid": True},
}

SIDE = 60           # pixels on a side; small, because what is held is the path, not the size
PLATEAU, TOWER = 800.4, 1234.6


def heights(tower=TOWER):
    """A plateau with a one-pixel tower, which is what max-pooling has to keep."""

    values = np.full((SIDE, SIDE), PLATEAU, dtype="float32")
    values[SIDE // 2, SIDE // 2] = tower
    return values


def ascii_grid(path: Path, delivery, tower) -> Path:
    """One ESRI ASCII grid, with the header Trentino's own files carry and no CRS.

    The first pixel centre is half a cell in from the corner, which is what `XLLCENTER`
    means, and nothing in the file names the grid it is on.
    """

    east, north = delivery["corner"]
    step = delivery["step"]
    rows = "\n".join(" ".join(f"{value:.2f}" for value in row)
                     for row in heights(tower))
    path.write_text(
        f"NCOLS {SIDE}\nNROWS {SIDE}\n"
        f"XLLCENTER {east + step / 2:.3f}\nYLLCENTER {north + step / 2:.3f}\n"
        f"CELLSIZE {step:.4f}\nNODATA_VALUE {delivery['nodata']:.0f}\n{rows}\n",
        encoding="utf-8")
    return path


def geotiff(path: Path, delivery, tower) -> Path:
    east, north = delivery["corner"]
    step = delivery["step"]
    with rasterio.open(path, "w", driver="GTiff", width=SIDE, height=SIDE, count=1,
                       dtype=delivery["dtype"], crs=CRS.from_epsg(delivery["epsg"]),
                       nodata=delivery["nodata"],
                       transform=Affine(step, 0, east, 0, -step, north + SIDE * step)) as dst:
        dst.write(heights(tower), 1)
    return path


def deliver(directory: Path, delivery, tower=TOWER) -> Path:
    """The portal's delivery on disk: a raster, a grid, or a zip holding one."""

    written = (ascii_grid if delivery.get("grid") else geotiff)(
        directory / delivery["name"], delivery, tower)
    if not delivery.get("zip"):
        return written
    bundle = directory / delivery["zip"]
    with zipfile.ZipFile(bundle, "w") as archive:
        archive.write(written, f"Wollongong_2020/{delivery['name']}")
        archive.writestr("Wollongong_2020/metadata.xml", "<licence>CC BY 4.0</licence>")
    written.unlink()
    return bundle


def arguments(key, archive, bbox, inputs):
    """The parsed arguments a subcommand runs with, as the CLI's parser builds them."""

    return argparse.Namespace(source=key, archive=str(archive), bbox=bbox,
                              input=str(inputs) if inputs else None, work=None)


def parse(bbox: str):
    return tuple(float(part) for part in bbox.split(","))


def box_of(delivery, pad=0.0002) -> str:
    """The delivery's own extent as a `--bbox`, worked out from the grid it is on.

    A `.asc` carries no CRS until the adapter places it, so the box cannot be read back
    out of the file the way the other tests read it.
    """

    east, north = delivery["corner"]
    side = SIDE * delivery["step"]
    to_wgs84 = Transformer.from_crs(delivery["epsg"], 4326, always_xy=True)
    west, south = to_wgs84.transform(east, north)
    right, top = to_wgs84.transform(east + side, north + side)
    return f"{west - pad},{south - pad},{right + pad},{top + pad}"


class Deliveries(unittest.TestCase):
    """What `--input` makes of what each portal hands over."""

    def setUp(self):
        self.work = TemporaryDirectory()
        self.root = Path(self.work.name)
        self.addCleanup(self.work.cleanup)

    def ingest(self, key, delivery, inputs=None, archive=None, work=None):
        if inputs is None:
            inputs = self.root / key / "delivery"
            inputs.mkdir(parents=True)
            deliver(inputs, delivery)
        archive = archive or self.root / key / "archive"
        datum = ingest.SOURCES[key].confirm_datum
        code = ingest.main(["ingest", key, "--bbox", box_of(delivery),
                            "--archive", str(archive), "--input", str(inputs),
                            "--work", str(work or self.root / key / "work")]
                           + (["--datum", datum] if datum else []))
        self.assertEqual(code, 0)
        return archive

    def test_every_portals_own_format_becomes_an_archive_tile(self):
        """One synthetic delivery per source, in the format the README documents.

        The tower is what the assertion is about: the delivery went through the shared
        tail, so the archive holds the maximum of the source pixels and not a resampling
        of them, whatever CRS, dtype and packaging the portal used.
        """

        for key, delivery in DELIVERIES.items():
            with self.subTest(key):
                archive = self.ingest(key, delivery)
                index = ingest.read_index(archive)
                self.assertEqual(set(index["sources"]), {key})
                # The index carries the credit a published map has to show, which for one
                # agency names the month the data was fetched.
                facts = index["sources"][key]
                self.assertEqual(facts["attribution"],
                                 ingest.SOURCES[key].credit(facts["fetched"]))
                self.assertNotIn("{", facts["attribution"])
                highest = None
                for tile in index["tiles"]:
                    ti, tj = (int(part) for part in tile.split("/"))
                    with rasterio.open(ingest.tile_path(archive, ti, tj)) as src:
                        data = src.read(1)
                    seen = data[data != ingest.NODATA]
                    if seen.size:
                        highest = max(highest or -32768, int(seen.max()))
                self.assertEqual(highest, round(TOWER))

    def test_an_ascii_grid_gets_its_crs_in_the_work_dir_and_the_delivery_is_untouched(self):
        """The CRS an ESRI ASCII grid arrives without is written where the tool may write.

        A delivery directory is the owner's download, so nothing goes into it: the grid is
        copied into the work directory and the `.prj` is written there.
        """

        delivery = DELIVERIES["it-tn"]
        inputs = self.root / "tn"
        inputs.mkdir()
        grid = deliver(inputs, delivery)
        before = sorted(path.name for path in inputs.iterdir())
        work = self.root / "work"
        kept = ingest.local_rasters(ingest.SOURCES["it-tn"], inputs,
                                    parse(box_of(delivery)), work)

        self.assertEqual(sorted(path.name for path in inputs.iterdir()), before)
        with rasterio.open(grid) as src:
            self.assertIsNone(src.crs)
        self.assertEqual(len(kept), 1)
        self.assertEqual(kept[0].parent, work)
        with rasterio.open(kept[0]) as src:
            self.assertEqual(src.crs, CRS.from_epsg(25832))

    def test_a_prj_the_portal_shipped_is_used_as_it_is(self):
        """A delivery that states its own grid is believed, and nothing is written."""

        delivery = DELIVERIES["it-tn"]
        inputs = self.root / "with-prj"
        inputs.mkdir()
        grid = deliver(inputs, delivery)
        grid.with_suffix(".prj").write_text(CRS.from_epsg(32632).to_wkt(), encoding="utf-8")
        kept = ingest.local_rasters(ingest.SOURCES["it-tn"], inputs,
                                    parse(box_of(delivery)), self.root / "work")
        self.assertEqual(kept, [grid])
        with rasterio.open(grid) as src:
            self.assertEqual(src.crs, CRS.from_epsg(32632))

    def test_a_re_issued_delivery_of_the_same_name_reaches_the_archive(self):
        """A portal re-uses a file name freely, so the name cannot be the identity.

        The second order holds a different tower. Keyed on the name alone, its members
        would be skipped as already unpacked and the archive would keep the first one.
        """

        delivery = DELIVERIES["au"]
        inputs = self.root / "orders"
        inputs.mkdir()
        archive = self.root / "twice" / "archive"
        work = self.root / "twice" / "work"
        higher = TOWER + 200.0

        deliver(inputs, delivery)
        self.ingest("au", delivery, inputs=inputs, archive=archive, work=work)
        first = ingest.read_index(archive)["sha256"]

        (inputs / delivery["zip"]).unlink()
        deliver(inputs, delivery, tower=higher)
        self.ingest("au", delivery, inputs=inputs, archive=archive, work=work)

        second = ingest.read_index(archive)["sha256"]
        self.assertEqual(set(first), set(second))
        self.assertNotEqual(first, second)
        for tile in second:
            ti, tj = (int(part) for part in tile.split("/"))
            with rasterio.open(ingest.tile_path(archive, ti, tj)) as src:
                data = src.read(1)
            self.assertEqual(int(data.max()), round(higher))

    def test_an_ascii_grid_of_a_row_that_states_no_grid_is_refused_by_name(self):
        """A `.prj` guessed wrong puts a mountain in the wrong country, so it is a
        refusal and not a default."""

        delivery = DELIVERIES["it-tn"]
        inputs = self.root / "loose"
        inputs.mkdir()
        deliver(inputs, delivery)
        with self.assertRaises(ingest.Refuse) as refusal:
            ingest.local_rasters(ingest.SOURCES["au"], inputs,
                                 parse(box_of(DELIVERIES["it-tn"])), self.root / "work")
        self.assertIn("names no CRS", str(refusal.exception))

    def test_a_zip_of_documents_beside_the_dem_is_an_ordinary_delivery(self):
        """An order ships its paperwork as a zip too, and the data is what matters.

        A zip inside the zip is only a refusal when this level holds no raster at all.
        """

        delivery = DELIVERIES["au"]
        inputs = self.root / "with-docs"
        inputs.mkdir()
        raster = geotiff(self.root / "dem.tif", delivery, TOWER)
        papers = self.root / "docs.zip"
        with zipfile.ZipFile(papers, "w") as bundle:
            bundle.writestr("readme.txt", "licence")
        with zipfile.ZipFile(inputs / delivery["zip"], "w") as bundle:
            bundle.write(raster, "DEM/dem.tif")
            bundle.write(papers, "metadata/docs.zip")

        kept = ingest.local_rasters(ingest.SOURCES["au"], inputs, parse(box_of(delivery)),
                                    self.root / "work")
        self.assertEqual([path.name for path in kept], ["dem.tif"])

    def test_a_zip_that_holds_only_another_zip_is_refused_by_its_name(self):
        delivery = DELIVERIES["au"]
        inputs = self.root / "only-nested"
        inputs.mkdir()
        inner = self.root / "inner.zip"
        with zipfile.ZipFile(inner, "w") as bundle:
            bundle.writestr("readme.txt", "licence")
        with zipfile.ZipFile(inputs / delivery["zip"], "w") as bundle:
            bundle.write(inner, "orders/inner.zip")
        with self.assertRaises(ingest.Refuse) as refusal:
            ingest.local_rasters(ingest.SOURCES["au"], inputs, parse(box_of(delivery)),
                                 self.root / "work")
        self.assertIn("orders/inner.zip", str(refusal.exception))

    def test_a_work_directory_inside_the_delivery_is_refused(self):
        """The tool writes into the work directory, so it cannot be the owner's download."""

        delivery = DELIVERIES["dk"]
        inputs = self.root / "dk" / "delivery"
        inputs.mkdir(parents=True)
        deliver(inputs, delivery)
        for work in (inputs, inputs / "unpacked"):
            with self.subTest(str(work)):
                code = ingest.main(["ingest", "dk", "--bbox", box_of(delivery),
                                    "--archive", str(self.root / "a"),
                                    "--input", str(inputs), "--work", str(work)])
                self.assertEqual(code, 1)

    def test_a_directory_that_holds_nothing_the_portal_delivers_says_so(self):
        inputs = self.root / "empty"
        inputs.mkdir()
        (inputs / "readme.txt").write_text("licence", encoding="utf-8")
        with self.assertRaises(ingest.Refuse) as refusal:
            ingest.local_rasters(ingest.SOURCES["dk"], inputs, (9.0, 55.0, 9.1, 55.1),
                                 self.root / "work")
        self.assertIn(".tif, .asc or .zip", str(refusal.exception))


class Credentials(unittest.TestCase):
    """What a source behind an account does with the environment, and without it."""

    def setUp(self):
        for key in ("DK", "FI", "SE"):
            for suffix in ("TOKEN", "USER", "PASSWORD"):
                name = f"OBC_REFERENCE_{key}_{suffix}"
                if name in os.environ:
                    real = os.environ.pop(name)
                    self.addCleanup(os.environ.__setitem__, name, real)

    def set(self, name, value):
        os.environ[name] = value
        self.addCleanup(os.environ.pop, name, None)

    def test_a_fetch_without_the_credential_names_the_variable_and_the_wizard(self):
        for key, expected in (("dk", "OBC_REFERENCE_DK_TOKEN"),
                              ("fi", "OBC_REFERENCE_FI_TOKEN"),
                              ("se", "OBC_REFERENCE_SE_USER and OBC_REFERENCE_SE_PASSWORD")):
            with self.subTest(key):
                with self.assertRaises(ingest.Refuse) as refusal:
                    ingest.SOURCES[key].require_credential()
                message = str(refusal.exception)
                self.assertIn(expected, message)
                self.assertIn("--input", message)
                self.assertIn(f"wizard {key}", message)

    def test_half_a_credential_is_no_credential(self):
        """Sweden wants two variables, and one of them is not a login."""

        self.set("OBC_REFERENCE_SE_USER", "consumer")
        with self.assertRaises(ingest.Refuse):
            ingest.SOURCES["se"].require_credential()
        self.set("OBC_REFERENCE_SE_PASSWORD", "secret")
        ingest.SOURCES["se"].require_credential()

    def test_a_token_rides_in_the_query_and_a_user_rides_in_the_header(self):
        """The two shapes these portals read a credential from, and nothing else."""

        self.set("OBC_REFERENCE_DK_TOKEN", "a token/with=signs")
        self.set("OBC_REFERENCE_SE_USER", "consumer")
        self.set("OBC_REFERENCE_SE_PASSWORD", "secret")

        denmark = ingest.SOURCES["dk"]
        self.assertEqual(denmark.credential.query(), "&token=a+token%2Fwith%3Dsigns")
        self.assertEqual(denmark.headers_for("https://api.dataforsyningen.dk/x"), {})

        sweden = ingest.SOURCES["se"]
        self.assertEqual(sweden.credential.query(), "")
        self.assertEqual(sweden.headers_for("https://dl1.lantmateriet.se/x"),
                         {"Authorization": "Basic Y29uc3VtZXI6c2VjcmV0"})

    def test_a_keyed_service_sends_the_protocols_request_plus_the_token(self):
        """A credential must not change the request: it is one parameter more."""

        self.set("OBC_REFERENCE_FI_TOKEN", "uuid-shaped")
        finland = ingest.SOURCES["fi"]
        box = (25.0, 68.0, 25.01, 68.005)
        sent = []
        real = ingest.sources.protocols.raster_bytes
        self.addCleanup(setattr, ingest.sources.protocols, "raster_bytes", real)
        ingest.sources.protocols.raster_bytes = lambda url, what: sent.append(url) or b"II*\x00"
        finland.request(box)
        self.assertEqual(sent[0], finland.url(box) + "&api-key=uuid-shaped")
        self.assertIn("coverageId=korkeusmalli_2m", sent[0])

    def answer_index(self, href):
        """The STAC search answers with one item, whose asset is at `href`."""

        seen = []
        real = ingest.sources.stac.http_get
        self.addCleanup(setattr, ingest.sources.stac, "http_get", real)

        def get(url, headers=None, what=None):
            seen.append((url, headers))
            return (b'{"features": [{"id": "a", "assets": {"data": {"href": "'
                    + href.encode() + b'"}}}]}')

        ingest.sources.stac.http_get = get
        return seen

    def catch_downloads(self):
        sent = []
        real = ingest.sources.bulk.http_download
        self.addCleanup(setattr, ingest.sources.bulk, "http_download", real)

        def download(url, path, optional=False, headers=None, what=None):
            sent.append((url, headers, what))
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"II*\x00")
            return path

        ingest.sources.bulk.http_download = download
        return sent

    def test_swedens_index_is_open_and_only_the_download_is_signed(self):
        """The one source whose index and whose data are on different sides of a login."""

        self.set("OBC_REFERENCE_SE_USER", "consumer")
        self.set("OBC_REFERENCE_SE_PASSWORD", "secret")
        sweden = ingest.SOURCES["se"]
        # The real index names this host for a download; the search itself is open.
        index = self.answer_index("https://dl1.lantmateriet.se/hojd/data/grid/mhm/75_6/m1.tif")
        self.assertEqual(sweden.files((18.4, 67.8, 18.5, 67.9)),
                         [("m1.tif", "https://dl1.lantmateriet.se/hojd/data/grid/mhm/75_6/m1.tif")])
        self.assertIsNone(index[0][1])

        sent = self.catch_downloads()
        with TemporaryDirectory() as work:
            sweden.fetch((18.4, 67.8, 18.5, 67.9), Path(work))
        self.assertEqual(sent[0][1], {"Authorization": "Basic Y29uc3VtZXI6c2VjcmV0"})
        # The label a refusal would carry is the source and the file, never the URL.
        self.assertEqual(sent[0][2], "se m1.tif")

    def test_an_index_that_names_another_host_does_not_get_the_password(self):
        """A STAC answer is data, not code. A `href` anywhere else is a refusal, and not
        a download that quietly goes out unsigned."""

        self.set("OBC_REFERENCE_SE_USER", "consumer")
        self.set("OBC_REFERENCE_SE_PASSWORD", "secret")
        sweden = ingest.SOURCES["se"]
        self.answer_index("https://dl1.lantmateriet.se.attacker.example/m1.tif")
        sent = self.catch_downloads()
        with self.assertRaises(ingest.Refuse) as refusal:
            with TemporaryDirectory() as work:
                sweden.fetch((18.4, 67.8, 18.5, 67.9), Path(work))
        message = str(refusal.exception)
        self.assertIn("dl1.lantmateriet.se.attacker.example", message)
        self.assertIn("lantmateriet.se", message)
        self.assertEqual(sent, [])
        self.assertNotIn("secret", message)

    def test_an_index_that_names_a_plain_http_download_does_not_get_the_password(self):
        """HTTP Basic is the password in clear text, so the scheme is not negotiable."""

        self.set("OBC_REFERENCE_SE_USER", "consumer")
        self.set("OBC_REFERENCE_SE_PASSWORD", "secret")
        sweden = ingest.SOURCES["se"]
        with self.assertRaises(ingest.Refuse) as refusal:
            sweden.headers_for("http://dl1.lantmateriet.se/hojd/data/m1.tif")
        message = str(refusal.exception)
        self.assertIn("not https", message)
        self.assertNotIn("secret", message)

    def test_a_row_whose_adapter_cannot_carry_its_credential_is_refused(self):
        """A credential the fetch path never reads is a request that goes out unsigned."""

        with self.assertRaises(ingest.Refuse) as refusal:
            ingest.sources.bulk.BulkSource(
                "xx", "Nowhere", "p", 1.0, "l", "a", "NAP", (0, 0, 1, 1),
                credential=ingest.Credential("xx", "token"))
        self.assertIn("cannot be carried by BulkSource", str(refusal.exception))

        with self.assertRaises(ingest.Refuse) as refusal:
            ingest.sources.bulk.BulkSource(
                "xx", "Nowhere", "p", 1.0, "l", "a", "NAP", (0, 0, 1, 1),
                credential=ingest.Credential("xx"))
        self.assertIn("needs credential_hosts", str(refusal.exception))


class Redaction(unittest.TestCase):
    """A refusal quotes what came off the wire, so it must not quote the token.

    The secret holds a `/`, an `=` and a `+`, because a token rides in a URL and a server
    that echoes the request echoes it percent-escaped: the plain form would not match.
    """

    SECRET = "tok/en=with+specials"

    def setUp(self):
        os.environ["OBC_REFERENCE_FI_TOKEN"] = self.SECRET
        self.addCleanup(os.environ.pop, "OBC_REFERENCE_FI_TOKEN", None)
        self.source = ingest.SOURCES["fi"]
        self.box = (25.0, 68.0, 25.01, 68.005)

    def failing(self, error):
        real = ingest.sources.base.OPENER
        self.addCleanup(setattr, ingest.sources.base, "OPENER", real)

        class Opener:
            def open(self, request, timeout=None):
                raise error

        ingest.sources.base.OPENER = Opener()

    def refusal_for(self, error):
        real = ingest.sources.base.RETRY_DELAYS
        self.addCleanup(setattr, ingest.sources.base, "RETRY_DELAYS", real)
        ingest.sources.base.RETRY_DELAYS = ()
        self.failing(error)
        with self.assertRaises(ingest.Refuse) as refusal:
            self.source.request(self.box)
        return str(refusal.exception)

    def quoting_error(self, code):
        """A service that answers by quoting the request it refused, token and all.

        The token is quoted as it travelled, which is percent-escaped, because that is
        what a server echoing a URL gives back.
        """

        body = io.BytesIO(f"refused: {self.source.url(self.box)}"
                          f"{self.source.credential.query()}".encode())
        return urllib.error.HTTPError("http://x", code, "no", {}, body)

    def assert_clean(self, message):
        """No form of the credential is left in the message, plain or escaped."""

        for form in (self.SECRET, urllib.parse.quote(self.SECRET, safe=""),
                     urllib.parse.quote_plus(self.SECRET)):
            self.assertNotIn(form, message)

    def test_a_4xx_that_quotes_the_request_does_not_quote_the_token(self):
        message = self.refusal_for(self.quoting_error(403))
        self.assert_clean(message)
        self.assertIn("<redacted>", message)
        self.assertIn("fi (25.0", message)

    def test_a_5xx_after_the_retries_does_not_quote_the_token(self):
        message = self.refusal_for(self.quoting_error(503))
        self.assert_clean(message)
        self.assertIn("after 0 retries", message)

    def test_an_xml_error_with_a_200_does_not_quote_the_token_either(self):
        """These services answer an error as a 200 and an XML document that quotes the
        request, so the body is a message like any other."""

        real = ingest.sources.protocols.http_get
        self.addCleanup(setattr, ingest.sources.protocols, "http_get", real)
        escaped = urllib.parse.quote(self.SECRET, safe="")
        quoted = f"<ExceptionReport>bad token in api-key={escaped}</ExceptionReport>"
        ingest.sources.protocols.http_get = lambda url, what=None: quoted.encode()
        with self.assertRaises(ingest.Refuse) as refusal:
            self.source.request(self.box)
        message = str(refusal.exception)
        self.assert_clean(message)
        self.assertIn("<redacted>", message)
        self.assertIn("did not answer with a TIFF", message)

    def test_a_dropped_connection_names_the_source_and_not_the_url(self):
        message = self.refusal_for(urllib.error.URLError(
            f"cannot reach {self.source.url(self.box)}{self.source.credential.query()}"))
        self.assert_clean(message)
        self.assertIn("fi (25.0", message)

    def test_a_credential_too_short_to_look_like_one_is_redacted_all_the_same(self):
        """A short credential is still a credential, so there is no length floor."""

        os.environ["OBC_REFERENCE_FI_TOKEN"] = "ab"
        self.assertIn("ab", ingest.sources.base.secrets())
        self.assertEqual(ingest.sources.base.redact("token=ab"), "token=<redacted>")


class RedirectHandler(BaseHTTPRequestHandler):
    """`/start` sends the client to the same server under its other name."""

    seen: list = []

    def do_GET(self):  # noqa: N802 — the name is the base class's
        self.seen.append((self.path, self.headers.get("Authorization")))
        if self.path == "/start":
            self.send_response(302)
            self.send_header("Location", f"http://localhost:{self.server.server_port}/end")
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("Content-Length", "4")
        self.end_headers()
        self.wfile.write(b"II*\x00")

    def log_message(self, *args):
        pass


class Redirects(unittest.TestCase):
    """A credential goes to the host the row named, and follows a redirect nowhere."""

    def setUp(self):
        RedirectHandler.seen = []
        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), RedirectHandler)
        Thread(target=self.httpd.serve_forever, daemon=True).start()
        self.addCleanup(self.httpd.server_close)
        self.addCleanup(self.httpd.shutdown)
        self.port = self.httpd.server_address[1]

    def test_a_redirect_to_another_host_drops_the_authorization(self):
        """urllib copies a request's headers onto the redirect it follows, so a portal
        that sent its download to a content network would hand it the password."""

        body = ingest.sources.base.http_get(
            f"http://127.0.0.1:{self.port}/start",
            headers={"Authorization": "Basic Y29uc3VtZXI6c2VjcmV0"})
        self.assertEqual(body, b"II*\x00")
        self.assertEqual([path for path, _ in RedirectHandler.seen], ["/start", "/end"])
        self.assertEqual(RedirectHandler.seen[0][1], "Basic Y29uc3VtZXI6c2VjcmV0")
        self.assertIsNone(RedirectHandler.seen[1][1])


class Wizard(unittest.TestCase):
    """The walk-through, driven by a list of answers instead of a person."""

    def setUp(self):
        self.work = TemporaryDirectory()
        self.root = Path(self.work.name)
        self.addCleanup(self.work.cleanup)
        for name in ("OBC_REFERENCE_DK_TOKEN", "OBC_REFERENCE_SE_USER",
                     "OBC_REFERENCE_SE_PASSWORD"):
            if name in os.environ:
                real = os.environ.pop(name)
                self.addCleanup(os.environ.__setitem__, name, real)

    def run_wizard(self, key, answers, inputs=None):
        self.said = []
        asked = []

        def ask(prompt):
            asked.append(prompt)
            return answers.pop(0) if answers else "q"

        code = ingest.command_wizard(
            arguments(key, self.root / key / "archive", box_of(DELIVERIES[key]), inputs),
            ask=ask, say=self.said.append)
        return code, asked

    def test_the_steps_are_walked_and_the_delivery_is_ingested(self):
        """Australia is the source with no fetch at all, so the wizard is the whole path.

        The answers are every step, the directory, `y` to ingest, and `y` to the datum
        the order's metadata states.
        """

        delivery = DELIVERIES["au"]
        inputs = self.root / "au" / "delivery"
        inputs.mkdir(parents=True)
        deliver(inputs, delivery)
        steps = len(ingest.SOURCES["au"].steps)
        code, asked = self.run_wizard("au", [""] * steps + [str(inputs), "y", "y"])
        self.assertEqual(code, 0)
        self.assertEqual(len(asked), steps + 3)
        self.assertIn("AHD", asked[-1])
        said = "\n".join(self.said)
        self.assertIn("Step 1 of", said)
        self.assertIn(f"Step {steps} of {steps}", said)
        self.assertIn("elevation.fsdf.org.au", said)
        self.assertIn(ingest.SOURCES["au"].attribution, said)
        self.assertIn(delivery["zip"], said)
        self.assertTrue(ingest.read_index(self.root / "au" / "archive")["tiles"])

    def test_a_delivery_the_owner_cannot_confirm_the_datum_of_is_not_ingested(self):
        """An ellipsoidal order is tens of metres out, which is the size of a lift."""

        delivery = DELIVERIES["au"]
        inputs = self.root / "au" / "delivery"
        inputs.mkdir(parents=True)
        deliver(inputs, delivery)
        steps = len(ingest.SOURCES["au"].steps)
        code, _ = self.run_wizard("au", [""] * steps + [str(inputs), "y", "n"])
        self.assertEqual(code, 1)
        self.assertIn("ellipsoidal height stands tens of metres", "\n".join(self.said))
        self.assertFalse((self.root / "au" / "archive").exists())

    def test_q_at_a_step_ingests_nothing(self):
        code, asked = self.run_wizard("au", ["", "q"])
        self.assertEqual(code, 1)
        self.assertEqual(len(asked), 2)
        self.assertIn("nothing was ingested", "\n".join(self.said))
        self.assertFalse((self.root / "au" / "archive").exists())

    def test_a_delivery_that_is_not_there_stops_before_the_ingest(self):
        steps = len(ingest.SOURCES["au"].steps)
        code, _ = self.run_wizard("au", [""] * steps + [str(self.root / "nowhere")])
        self.assertEqual(code, 1)
        self.assertIn("is not a directory", "\n".join(self.said))

    def test_a_pasted_credential_goes_into_the_environment_and_not_into_argv(self):
        """The wizard's own process runs the ingest, so the token needs no command line."""

        said, asked = [], []

        def ask(prompt):
            asked.append(prompt)
            return "pasted-token-value"

        self.assertTrue(ingest.wizard.take_credential(ingest.SOURCES["dk"], ask, said.append))
        self.assertEqual(asked, ["  OBC_REFERENCE_DK_TOKEN: "])
        self.assertEqual(os.environ["OBC_REFERENCE_DK_TOKEN"], "pasted-token-value")
        self.assertNotIn("pasted-token-value", "\n".join(said))

    def test_an_empty_answer_falls_back_to_the_download_by_hand(self):
        said = []
        self.assertFalse(ingest.wizard.take_credential(
            ingest.SOURCES["se"], lambda _: "", said.append))
        self.assertNotIn("OBC_REFERENCE_SE_USER", os.environ)
        self.assertIn("the download by hand it is", "\n".join(said))

    def test_a_credential_that_is_already_set_asks_nothing(self):
        os.environ["OBC_REFERENCE_DK_TOKEN"] = "a token"
        said = []
        self.assertTrue(ingest.wizard.take_credential(
            ingest.SOURCES["dk"], lambda _: self.fail("it asked"), said.append))
        self.assertIn("OBC_REFERENCE_DK_TOKEN is already set", "\n".join(said))

    def test_a_source_that_needs_no_account_says_to_ingest_it_directly(self):
        with self.assertRaises(ingest.Refuse) as refusal:
            ingest.command_wizard(arguments("nz", self.root / "a", "170,-44,170.1,-43.9", None),
                                  ask=lambda _: "", say=lambda _: None)
        self.assertIn("needs no account", str(refusal.exception))


if __name__ == "__main__":
    unittest.main()
