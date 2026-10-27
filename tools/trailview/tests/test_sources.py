"""The ground sources of ground.json: the legends' matchers, the combine rules, the satellite's heath,
the lithology chain, and one captured sample of each national map through its capture and reader."""

import json
import math
import tempfile
import unittest
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
            steps, zooms = sources.chain("lithology", GROUND, VOCABULARY, files.get, bounds)
            from tools.trailview import ground

            rock = ground.Lithology(steps)(z, x, y)
            self.assertEqual(sorted(path.name for path in root.iterdir()), ["33e6f843", "a1b2c3", "shp"], "the zip is read in place")
        self.assertEqual(zooms, [10, 10])
        column = lambda lon: int((lon - west) / (east - west) * tiles.TILE)
        self.assertEqual(rock[256, column(west + 5 * km)], L["chalk"])
        self.assertEqual(rock[256, column(west + 11 * km)], L["chalk"], "IGME 5000's limestone 1 km from EGDI's chalk")
        self.assertEqual(rock[256, column(west + 15 * km)], L["limestone"], "5 km from EGDI's chalk")


class CaptureTest(unittest.TestCase):
    """A captured answer of each service, through the capture and the vector reader."""

    def polygons(self, source, bounds, answer):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(sources, "get", side_effect=answer):
            sources.fetch(source, bounds, Path(tmp), 13, GROUND)
            files = {path.name: path for path in Path(tmp).iterdir()}
            entry = sources.entry_of(source, GROUND)
            layer = M if any(entry is e for e in GROUND["material"]["sources"]) else L
            return sources.vector_polygons(files, entry, bounds, sources.Legend(entry["legend"], layer))

    def arcgis(self, folder):
        return lambda url, params=None, data=None, **_: (CAPTURED / folder / ("ids.json" if data.get("returnIdsOnly") else "features.geojson")).read_bytes()

    def test_ireland_names_exposed_rock_and_wet_grassland(self):
        polygons = self.polygons("land-cover-ie", [-9.744, 51.998, -9.742, 51.999], self.arcgis("land-cover-ie"))
        self.assertEqual(sorted(polygons.codes), sorted([M["bare-rock"], M["bare-rock"], M["wet-grassland"]]))

    def test_scotland_names_dry_and_wet_heath(self):
        page = lambda url, params=None, **_: (CAPTURED / "land-cover-gb-sct" / "page.json").read_bytes()
        polygons = self.polygons("land-cover-gb-sct", [-3.230, 57.021, -3.226, 57.024], page)
        self.assertEqual(polygons.codes, [M["heath"], M["heath"]])

    def test_spain_names_limestone_and_no_rock_for_scree_or_a_formation_without_one(self):
        polygons = self.polygons("geology-es", [-4.856, 43.194, -4.855, 43.195], self.arcgis("geology-es"))
        self.assertEqual(polygons.codes, [L["limestone"]])


if __name__ == "__main__":
    unittest.main()
