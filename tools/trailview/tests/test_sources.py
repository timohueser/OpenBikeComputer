"""The ground sources of ground.json: the legends' matchers, the combine rules, the satellite's heath,
the lithology chain, and one captured sample of each national map through its capture and reader."""

import collections
import io
import json
import math
import tempfile
import unittest
import urllib.parse
import zipfile
from pathlib import Path
from unittest import mock

import numpy as np
import pyogrio
import shapely
from pyproj import Transformer

from tools import planner_geo as geo
from tools.trailview import sources, tiles

VOCABULARY = sources.Vocabulary(json.loads(Path("data/trailview/materials.json").read_text()))
M, L = VOCABULARY.material, VOCABULARY.lithology
GROUND = json.loads(Path("data/trailview/ground.json").read_text())
CAPTURED = Path("fixtures/sources/trailview-ground")


class LegendTest(unittest.TestCase):
    def test_each_matcher_finds_its_class_and_none_is_0(self):
        legend = lambda match, classes: sources.Legend({"match": match, "classes": classes}, M)
        code = legend("code", [["10", "forest"], ["dolomite/dolostone", "heath"], ["chalk", "bog"]])
        self.assertEqual([code(10), code("dolomite/dolostone"), code("http://inspire.ec.europa.eu/codelist/LithologyValue/chalk"),
                          code("11"), code(None)], [M["forest"], M["heath"], M["bog"], 0, 0])
        prefix = legend("prefix", [["F4", "heath"], ["E5", "rough-grass"], ["E5.3", "bracken"]])
        self.assertEqual([prefix("F4.11"), prefix("E5.31"), prefix("E5.1"), prefix("J1")], [M["heath"], M["bracken"], M["rough-grass"], 0])
        word = legend("word", [["bog", "bog"], ["heath", "heath"], ["wet heath", "scrub"]])
        self.assertEqual([word("Heath and bog"), word("Blanket Bog with heath"), word("Wet Heath"), word("Lake")],
                         [M["heath"], M["bog"], M["scrub"], 0], "the earliest word, the longest on a tie")
        qualified = sources.Legend({"match": "word", "classes": [["arcilla", "marl-clay"], ["caliza", "limestone"]],
                                    "qualifier": {"word": "abigarrad", "within": 3, "class": "variegated-marl"}}, L)
        self.assertEqual([qualified("Arcillas abigarradas"), qualified("Calizas y arcillas abigarradas"),
                          qualified("Tramos abigarrados"), qualified("Arcillas rojas abigarradas")],
                         [L["variegated-marl"], L["limestone"], L["variegated-marl"], L["marl-clay"]],
                         "the qualifier right after the earliest word, or alone")
        tags = legend("tags", [[{"natural": "wetland", "wetland": "bog"}, "bog"], [{"natural": "heath"}, "heath"]])
        self.assertEqual([tags({"natural": "wetland", "wetland": "bog"}), tags({"natural": "wetland"}), tags({"natural": "heath", "x": "y"})],
                         [M["bog"], 0, M["heath"]])


class CombineTest(unittest.TestCase):
    def test_an_authority_takes_what_it_names_but_the_water_and_a_refinement_only_what_it_names_more_precisely(self):
        fine = sources.refinements(GROUND["material"]["refines"], M)
        out = np.array([M["grassland"], M["inland-water"], M["forest"], M["heath"]], np.uint8)
        proposed = np.array([M["pasture"], M["pasture"], 0, M["pasture"]], np.uint8)
        self.assertEqual(sources.combine(out, proposed, "authority", fine, [M["inland-water"]]).tolist(),
                         [M["pasture"], M["inland-water"], M["forest"], M["pasture"]])
        proposed = np.array([M["pasture"], M["pasture"], M["broadleaf-forest"], M["bog"]], np.uint8)
        self.assertEqual(sources.combine(out, proposed, "refine", fine).tolist(),
                         [M["pasture"], M["inland-water"], M["broadleaf-forest"], M["bog"]])


    def test_an_osm_orchard_of_berries_takes_back_a_corine_orchard_and_nothing_else(self):
        fine = sources.refinements(GROUND["material"]["refines"], M)
        osm = [entry for entry in GROUND["material"]["sources"] if entry.get("protocol") == "osm"][-1]
        berries = sources.Legend(osm["legend"], M)({"landuse": "orchard", "trees": "blueberry_plants"})
        out = np.array([M["orchard"], M["forest"], M["grassland"]], np.uint8)
        self.assertEqual(sources.combine(out, np.full(3, berries, np.uint8), "refine", fine).tolist(),
                         [M["arable"], M["forest"], M["grassland"]])


class HeathTest(unittest.TestCase):
    def test_ground_that_greens_in_spring_is_heath_only_off_calcareous_rock(self):
        entry = next(entry for entry in GROUND["material"]["sources"] if entry.get("rule") == "satellite-heath")
        # Summer as red as green with an NDVI of 0.6; spring green. Texels: on limestone, on granite, on
        # limestone where spring saw snow, and scrub on granite.
        summer, spring = (150, 150, 120, 204), (100, 140, 80, 204)
        quarters = {q: np.array([[spring if q == 2 else summer] * 4], np.uint8) for q in range(1, 5)}
        quarters[2][0, 2, 3] = 1
        most = np.full((1, 4), 204, np.uint8)
        rock = np.array([[L["limestone"], L["granite"], L["limestone"], L["granite"]]], np.uint8)
        out = np.array([[M["grassland"], M["grassland"], M["grassland"], M["forest"]]], np.uint8)
        heath = sources.Heath(entry, VOCABULARY)(out, sources.Tile(None, None, quarters, most, rock))
        self.assertEqual(heath.tolist(), [[M["grassland"], M["heath"], M["heath"], M["forest"]]])


class LithologyTest(unittest.TestCase):
    def test_egdi_names_the_rock_over_igme_5000_and_its_chalk_refines_limestone_within_its_reach(self):
        z, x, y = 10, *map(int, geo.mercator(8.40, 46.82, 10))
        bounds = geo.tile_bounds(z, x, y)
        west, south, east, north = bounds
        km = 1 / (111.32 * math.cos(math.radians((south + north) / 2)))
        igme = next(entry for entry in GROUND["lithology"]["sources"] if entry["source"] == "igme5000")
        egdi = next(entry for entry in GROUND["lithology"]["sources"] if entry["source"] == "egdi-geology")
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(tempfile, "tempdir", tmp):
            root = Path(tmp)
            # IGME 5000, in its zip as the store holds it: limestone everywhere.
            x0, y0, x1, y1 = Transformer.from_crs("EPSG:4326", "EPSG:3034", always_xy=True).transform_bounds(west - 1, south - 1, east + 1, north + 1)
            (root / "shp").mkdir()
            pyogrio.raw.write(root / "shp" / f"{igme['layer']}.shp", geometry=shapely.to_wkb([shapely.box(x0, y0, x1, y1)]),
                              field_data=[np.array(["carbonates, consolidated"], object)], fields=[igme["field"]], crs="EPSG:3034",
                              geometry_type="Polygon", driver="ESRI Shapefile")
            archive = root / "33e6f843"
            with zipfile.ZipFile(archive, "w") as z_:
                for path in sorted((root / "shp").iterdir()):
                    z_.write(path, f"europe/data/{path.name}")
            # EGDI: chalk over the west 10 km of the tile, and a cover unit east of it.
            uri = "http://inspire.ec.europa.eu/codelist/LithologyValue/"
            gpkg = root / "a1b2c3"
            pyogrio.raw.write(gpkg, geometry=shapely.to_wkb([shapely.box(west - 1, south - 1, west + 10 * km, north + 1),
                                                             shapely.box(west + 10 * km, south - 1, east + 1, north + 1)]),
                              field_data=[np.array([uri + "chalk", uri + "clayWithFlints"], object)], fields=[egdi["field"]],
                              crs="EPSG:4326", geometry_type="Polygon", driver="GPKG", layer=egdi["layer"])
            files = {"igme5000": {"IGME5000.zip": archive}, "egdi-geology": {"GeologicUnitView.gpkg": gpkg}}
            steps = sources.chain("lithology", GROUND, VOCABULARY, files.get, bounds)
            from tools.trailview import ground

            rock = ground.Lithology(steps)(z, x, y)
            kept = sorted(path.name for path in root.iterdir() if not path.name.startswith("ground-"))
            self.assertEqual(kept, ["33e6f843", "a1b2c3", "shp"], "the zip is read in place, the units kept in the scratch")
        column = lambda lon: int((lon - west) / (east - west) * tiles.TILE)
        self.assertEqual(rock[256, column(west + 5 * km)], L["chalk"])
        self.assertEqual(rock[256, column(west + 11 * km)], L["chalk"], "IGME 5000's limestone 1 km from EGDI's chalk")
        self.assertEqual(rock[256, column(west + 15 * km)], L["limestone"], "5 km from EGDI's chalk")


def playing(service):
    """The services of a capture played by `service(params)`, the query and form of a request, which
    gives a body or raises; no wait between tries."""
    def open_url(request, timeout=60):
        params = dict(urllib.parse.parse_qsl(urllib.parse.urlsplit(request.full_url).query))
        return io.BytesIO(service(params | dict(urllib.parse.parse_qsl((request.data or b"").decode()))))
    return mock.patch("tools.planner_runtime.open_url", open_url), mock.patch.object(sources.time, "sleep")


class CaptureTest(unittest.TestCase):
    """A captured answer of each national map through its capture and the units of its source; and the
    capture's batches, pages, retries and resume against a service that a test plays."""

    def units(self, source, bounds, out):
        entry = sources.entry_of(source, GROUND)
        layer = M if any(entry is e for e in GROUND["material"]["sources"]) else L
        units = sources.Units({p.name: p for p in out.iterdir()}, entry, bounds, sources.Legend(entry["legend"], layer),
                              out.parent / "units.gpkg")
        return sorted(units.within((-2e7, -2e7, 2e7, 2e7))[1].tolist())

    def served(self, source, bounds, service):
        network, wait = playing(service)
        with tempfile.TemporaryDirectory() as tmp, network, wait:
            out = Path(tmp) / "out"
            sources.fetch(source, bounds, out, 13, GROUND)
            return self.units(source, bounds, out)

    def archived(self, source, bounds, answer, answered, unnamed, crs, name, driver):
        """The codes and pages of the capture of the bounds from the zip that the archive keeps: the
        features of a service's `answer`, whose class is in the field `answered`, after one that the
        legend does not name, and before one outside the bounds; two features to a page. A second
        capture reads the unpacked copy without the zip."""
        entry, features = sources.entry_of(source, GROUND), json.loads((CAPTURED / source / answer).read_text())["features"]
        values = [feature["properties"][answered] for feature in features]
        west, south, east, north = bounds
        geometries = [shapely.box(*bounds), *(shapely.geometry.shape(f["geometry"]) for f in features),
                      shapely.box(east + 0.01, south, east + 0.02, north)]
        to_map = Transformer.from_crs("EPSG:4326", crs, always_xy=True)
        projected = shapely.transform(np.array(geometries), lambda xy: np.stack(to_map.transform(xy[:, 0], xy[:, 1]), axis=-1))
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(sources, "ARCHIVE_PAGE", 2):
            tmp = Path(tmp)
            pyogrio.raw.write(str(tmp / name), geometry=shapely.to_wkb(projected), field_data=[np.array([unnamed, *values, values[0]], object)],
                              fields=[entry["field"]], layer=entry["layer"], driver=driver, geometry_type="MultiPolygon",
                              promote_to_multi=True, crs=crs)
            with zipfile.ZipFile(tmp / "map.zip", "w", zipfile.ZIP_DEFLATED) as z:
                z.writestr("metadata.html", "")
                for path in sorted((tmp / name).rglob("*")) if (tmp / name).is_dir() else [tmp / name]:
                    z.write(path, path.relative_to(tmp))
            out = tmp / "out"
            out.mkdir()
            (out / "features-00009.geojson").write_text("{}")
            capture = lambda: sources.fetch(source, bounds, out, 13, GROUND, archive=tmp / "map.zip", work=tmp / "work")
            pages = lambda: [[f["properties"] for f in json.loads(page.read_text())["features"]] for page in sorted(out.glob("features-*"))]
            capture()
            self.assertEqual(pages(), [[{entry["field"]: value} for value in values[i:i + 2]] for i in range(0, len(values), 2)],
                             "in id order, without the feature that the legend does not name, the one outside or the old page")
            first = pages()
            (tmp / "map.zip").unlink()
            capture()
            self.assertEqual(pages(), first, "a later capture reads the unpacked copy without the zip")
            return self.units(source, bounds, out)

    def arcgis(self, folder):
        return lambda params: (CAPTURED / folder / ("ids.json" if "returnIdsOnly" in params else "features.geojson")).read_bytes()

    def test_ireland_names_exposed_rock_and_wet_grassland(self):
        codes = self.archived("land-cover-ie", [-9.744, 51.998, -9.742, 51.999], "features.geojson", "LEVEL_2_VALUE",
                              "Buildings", "EPSG:2157", "map.gdb", "OpenFileGDB")
        self.assertEqual(codes, sorted([M["bare-rock"], M["bare-rock"], M["wet-grassland"]]))

    def test_scotland_names_dry_and_wet_heath(self):
        codes = self.archived("land-cover-gb-sct", [-3.230, 57.021, -3.226, 57.024], "page.json", "HABITAT_CODE", "J1",
                              "EPSG:27700", "map.gpkg", "GPKG")
        self.assertEqual(codes, [M["heath"], M["heath"]])

    def test_spain_names_limestone_and_variegated_marl_and_no_rock_where_a_unit_names_none(self):
        codes = self.served("geology-es", [-4.86, 43.19, -4.85, 43.20], self.arcgis("geology-es"))
        self.assertEqual(codes, sorted([L["limestone"], L["variegated-marl"]]))

    def test_an_arcgis_capture_filters_by_the_legend_retries_asks_again_past_the_transfer_limit_and_resumes(self):
        import http.client
        from urllib.error import HTTPError

        entry, asked, calls = sources.entry_of("geology-es", GROUND), [], []

        def service(params):
            calls.append(params)
            if len(calls) == 1:
                raise HTTPError("https://arcgis.example.org", 502, "Bad Gateway", {}, None)
            if len(calls) == 3:
                return b'{"error": {"code": 500, "message": "busy"}}'
            if len(calls) == 4:
                raise http.client.IncompleteRead(b"{")
            if "returnIdsOnly" in params:
                self.assertIn("LOWER(DESC_UNIT) LIKE '%calcaren%'", params["where"])
                # 999 is gone before the capture asks for it.
                return json.dumps({"objectIdFieldName": "OBJECTID", "objectIds": [*range(450, 0, -1), 999]}).encode()
            ids = [int(i) for i in params["objectIds"].split(",")]
            asked.append(ids)
            return json.dumps({"type": "FeatureCollection", "features": [
                {"type": "Feature", "geometry": None, "properties": {"OBJECTID": i, "DESC_UNIT": "Calizas"}}
                for i in ids[:150] if i != 999]}).encode()

        network, wait = playing(service)
        serial = mock.patch.object(sources, "ThreadPoolExecutor", lambda n: Serial())
        with tempfile.TemporaryDirectory() as tmp, network, wait, serial, mock.patch.object(sources, "VANISHED", 0.01):
            out = Path(tmp)
            (out / "features-00001.geojson").write_text('{"type": "FeatureCollection", "features": []}')
            sources.fetch_features("https://arcgis.example.org/0", entry, [-9.8, 51.9, -9.6, 52.1], out, 1e-5,
                                   sources.Legend(entry["legend"], L))
            pages = {path.name: [f["properties"]["OBJECTID"] for f in json.loads(path.read_text())["features"]]
                     for path in sorted(out.glob("features-*"))}
        self.assertEqual(pages["features-00000.geojson"], list(range(1, 201)), "asked again for the ids past the limit of 150")
        self.assertEqual(pages["features-00001.geojson"], [], "the page that a failed run kept stays")
        self.assertEqual(pages["features-00002.geojson"], list(range(401, 451)), "without the vanished id")
        self.assertEqual([ids[0] for ids in asked], [1, 151, 401, 999], "the kept page is not asked again")

    def test_an_arcgis_error_with_a_4xx_code_fails_at_once(self):
        calls = []

        def service(params):
            calls.append(params)
            return b'{"error": {"code": 400, "message": "Invalid where"}}'

        entry = sources.entry_of("geology-es", GROUND)
        network, wait = playing(service)
        with tempfile.TemporaryDirectory() as tmp, network, wait, self.assertRaises(sources.Refused):
            sources.fetch_features("https://arcgis.example.org/0", entry, [-9.8, 51.9, -9.6, 52.1], Path(tmp), 1e-5,
                                   sources.Legend(entry["legend"], L))
        self.assertEqual(len(calls), 1)

    def test_a_resumed_capture_drops_its_pages_when_their_positions_mean_other_features(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            sources.positions(out, "ids a")
            (out / "features-00000.geojson").write_text("{}")
            sources.positions(out, "ids a")
            self.assertTrue((out / "features-00000.geojson").exists())
            sources.positions(out, "ids b")
            self.assertFalse((out / "features-00000.geojson").exists())


class UnpackTest(unittest.TestCase):
    """The copy of an archived zip: a stopped unpack resumes, and it refuses a full disk and a zip of
    other files."""

    def zip(self, tmp, members):
        with zipfile.ZipFile(tmp / "map.zip", "w", zipfile.ZIP_DEFLATED) as z:
            for name, body in members.items():
                z.writestr(name, body)
        return tmp / "map.zip"

    def test_a_stopped_unpack_resumes_with_the_members_in_place_after_a_check_of_the_free_space(self):
        Usage = collections.namedtuple("Usage", "total used free")
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            archive = self.zip(tmp, {"map.gdb/a": b"aaaa", "map.gdb/b": b"bbbbbb", "readme.html": b""})
            work = tmp / "copy"
            (work / "map.gdb").mkdir(parents=True)
            (work / "map.gdb/a").write_bytes(b"xxxx")
            (work / "map.gdb/b.part").write_bytes(b"bb")
            with mock.patch("shutil.disk_usage", return_value=Usage(0, 0, sources.RESERVE + 5)):
                self.assertRaisesRegex(RuntimeError, "needs 0.0 GiB more", sources.unpack, archive, work)
            self.assertEqual(sources.unpack(archive, work), str(work / "map.gdb"))
            self.assertEqual((work / "map.gdb/a").read_bytes(), b"xxxx", "a member in place stays")
            self.assertEqual((work / "map.gdb/b").read_bytes(), b"bbbbbb")
            self.assertTrue((work / ".complete").is_file())
            self.assertFalse((work / "readme.html").exists())

    def test_a_zip_without_one_geopackage_or_filegdb_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            archive = self.zip(Path(tmp), {"a.gpkg": b"a", "b.gpkg": b"b"})
            self.assertRaisesRegex(ValueError, "not one GeoPackage or FileGDB", sources.unpack, archive, Path(tmp) / "copy")


class Serial:
    """A pool that runs each call in turn, so a played service sees one order."""

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False

    def map(self, function, items):
        return map(function, items)


if __name__ == "__main__":
    unittest.main()
