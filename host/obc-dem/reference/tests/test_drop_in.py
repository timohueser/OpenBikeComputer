"""A source behind an account: the delivered files, the refusal, and the wizard.

No test here reaches the network or wants a credential. What the portals deliver is built
synthetically, in the format `README.md` documents for each one — the name pattern, the
CRS, the dtype and the nodata value — and put through `ingest --input`, because that path
is the whole adapter for a source nobody can fetch unattended. The live probes are in
`README.md`.
"""

import argparse
import os
import sys
import unittest
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


def heights():
    """A plateau with a one-pixel tower, which is what max-pooling has to keep."""

    values = np.full((SIDE, SIDE), PLATEAU, dtype="float32")
    values[SIDE // 2, SIDE // 2] = TOWER
    return values


def ascii_grid(path: Path, delivery) -> Path:
    """One ESRI ASCII grid, with the header Trentino's own files carry and no CRS.

    The first pixel centre is half a cell in from the corner, which is what `XLLCENTER`
    means, and nothing in the file names the grid it is on.
    """

    east, north = delivery["corner"]
    step = delivery["step"]
    rows = "\n".join(" ".join(f"{value:.2f}" for value in row) for row in heights())
    path.write_text(
        f"NCOLS {SIDE}\nNROWS {SIDE}\n"
        f"XLLCENTER {east + step / 2:.3f}\nYLLCENTER {north + step / 2:.3f}\n"
        f"CELLSIZE {step:.4f}\nNODATA_VALUE {delivery['nodata']:.0f}\n{rows}\n",
        encoding="utf-8")
    return path


def geotiff(path: Path, delivery) -> Path:
    east, north = delivery["corner"]
    step = delivery["step"]
    with rasterio.open(path, "w", driver="GTiff", width=SIDE, height=SIDE, count=1,
                       dtype=delivery["dtype"], crs=CRS.from_epsg(delivery["epsg"]),
                       nodata=delivery["nodata"],
                       transform=Affine(step, 0, east, 0, -step, north + SIDE * step)) as dst:
        dst.write(heights(), 1)
    return path


def deliver(directory: Path, delivery) -> Path:
    """The portal's delivery on disk: a raster, a grid, or a zip holding one."""

    written = (ascii_grid if delivery.get("grid") else geotiff)(
        directory / delivery["name"], delivery)
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

    def ingest(self, key, delivery):
        inputs = self.root / key / "delivery"
        inputs.mkdir(parents=True)
        deliver(inputs, delivery)
        archive = self.root / key / "archive"
        code = ingest.main(["ingest", key, "--bbox", box_of(delivery),
                            "--archive", str(archive), "--input", str(inputs),
                            "--work", str(self.root / key / "work")])
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
                self.assertEqual(index["sources"][key]["attribution"],
                                 ingest.SOURCES[key].attribution)
                highest = None
                for tile in index["tiles"]:
                    ti, tj = (int(part) for part in tile.split("/"))
                    with rasterio.open(ingest.tile_path(archive, ti, tj)) as src:
                        data = src.read(1)
                    seen = data[data != ingest.NODATA]
                    if seen.size:
                        highest = max(highest or -32768, int(seen.max()))
                self.assertEqual(highest, round(TOWER))

    def test_an_ascii_grid_is_given_the_crs_its_format_cannot_carry(self):
        """The adapter writes the `.prj`, so the tail sees an ordinary placed raster."""

        delivery = DELIVERIES["it-tn"]
        inputs = self.root / "tn"
        inputs.mkdir()
        grid = deliver(inputs, delivery)
        with rasterio.open(grid) as src:
            self.assertIsNone(src.crs)
        ingest.local_rasters(ingest.SOURCES["it-tn"], inputs, (10.86, 46.15, 10.88, 46.17),
                             self.root / "work")
        with rasterio.open(grid) as src:
            self.assertEqual(src.crs, CRS.from_epsg(25832))

    def test_an_ascii_grid_of_a_row_that_states_no_grid_is_refused_by_name(self):
        """A `.prj` guessed wrong puts a mountain in the wrong country, so it is a
        refusal and not a default."""

        delivery = DELIVERIES["it-tn"]
        inputs = self.root / "loose"
        inputs.mkdir()
        deliver(inputs, delivery)
        with self.assertRaises(ingest.Refuse) as refusal:
            ingest.local_rasters(ingest.SOURCES["au"], inputs, (10.86, 46.15, 10.88, 46.17),
                                 self.root / "work")
        self.assertIn("names no CRS", str(refusal.exception))

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
        query = denmark.credential.query()
        self.assertEqual(query, "&token=a+token%2Fwith%3Dsigns")
        self.assertEqual(denmark.headers(), {})

        sweden = ingest.SOURCES["se"]
        self.assertEqual(sweden.credential.query(), "")
        self.assertEqual(sweden.headers(), {"Authorization": "Basic Y29uc3VtZXI6c2VjcmV0"})

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

    def test_swedens_index_is_open_and_only_the_download_is_signed(self):
        """The one source whose index and whose data are on different sides of a login."""

        self.set("OBC_REFERENCE_SE_USER", "consumer")
        self.set("OBC_REFERENCE_SE_PASSWORD", "secret")
        sweden = ingest.SOURCES["se"]

        index = []
        real_get = ingest.sources.stac.http_get
        self.addCleanup(setattr, ingest.sources.stac, "http_get", real_get)

        def get(url, headers=None):
            index.append((url, headers))
            return b'{"features": [{"id": "a", "assets": {"data": {"href": "https://d/m1.tif"}}}]}'

        ingest.sources.stac.http_get = get
        self.assertEqual(sweden.files((18.4, 67.8, 18.5, 67.9)), [("m1.tif", "https://d/m1.tif")])
        self.assertIsNone(index[0][1])

        downloads = []
        real_download = ingest.sources.bulk.http_download
        self.addCleanup(setattr, ingest.sources.bulk, "http_download", real_download)

        def download(url, path, optional=False, headers=None):
            downloads.append((url, headers))
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"II*\x00")
            return path

        ingest.sources.bulk.http_download = download
        with TemporaryDirectory() as work:
            sweden.fetch((18.4, 67.8, 18.5, 67.9), Path(work))
        self.assertEqual(downloads[0][1], {"Authorization": "Basic Y29uc3VtZXI6c2VjcmV0"})


class Wizard(unittest.TestCase):
    """The walk-through, driven by a list of answers instead of a person."""

    def setUp(self):
        self.work = TemporaryDirectory()
        self.root = Path(self.work.name)
        self.addCleanup(self.work.cleanup)
        for name in ("OBC_REFERENCE_AU_TOKEN", "OBC_REFERENCE_DK_TOKEN"):
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
        """Australia is the source with no fetch at all, so the wizard is the whole path."""

        delivery = DELIVERIES["au"]
        inputs = self.root / "au" / "delivery"
        inputs.mkdir(parents=True)
        deliver(inputs, delivery)
        steps = len(ingest.SOURCES["au"].steps)
        code, asked = self.run_wizard("au", [""] * steps + [str(inputs), "y"])
        self.assertEqual(code, 0)
        self.assertEqual(len(asked), steps + 2)
        said = "\n".join(self.said)
        self.assertIn("Step 1 of", said)
        self.assertIn(f"Step {steps} of {steps}", said)
        self.assertIn("elevation.fsdf.org.au", said)
        self.assertIn(ingest.SOURCES["au"].attribution, said)
        self.assertIn(delivery["zip"], said)
        self.assertTrue(ingest.read_index(self.root / "au" / "archive")["tiles"])

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

    def test_a_credential_that_is_set_skips_the_download_by_hand(self):
        """The wizard's last step is the ingest, and with a token that is a live fetch."""

        os.environ["OBC_REFERENCE_DK_TOKEN"] = "a token"
        self.addCleanup(os.environ.pop, "OBC_REFERENCE_DK_TOKEN", None)
        said = []
        where = ingest.wizard.input_directory(ingest.SOURCES["dk"], None, lambda _: "", said.append)
        self.assertEqual(where, "")
        self.assertIn("OBC_REFERENCE_DK_TOKEN is set", "\n".join(said))

    def test_a_source_that_needs_no_account_says_to_ingest_it_directly(self):
        with self.assertRaises(ingest.Refuse) as refusal:
            ingest.command_wizard(arguments("nz", self.root / "a", "170,-44,170.1,-43.9", None),
                                  ask=lambda _: "", say=lambda _: None)
        self.assertIn("needs no account", str(refusal.exception))


if __name__ == "__main__":
    unittest.main()
