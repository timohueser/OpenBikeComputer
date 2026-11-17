"""A route source reads into the common form of specs/obc-data.md, by its description alone."""

import hashlib
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from tools import planner_route_sources as sources

DESCRIPTION = """
origin = 7
countries = ["CH"]
kind = "hiking"

[route]
layer = "Route"
key = "Nr"
ref = "{Nr}"
name = "{Name}"
rank = { field = "Level", values = { national = 3, local = 1 } }
ascent = "Up"
keep = { Way = ["out", "both"] }

[stage]
layer = "Stage"
of = "Nr"
number = "Stage"
name = "{route}, {From} – {To}"
keep = { Way = ["out", "both"] }
"""
BOUNDS = [8.0, 47.0, 8.1, 47.1]


def lv95(lon, lat):
    import pyproj

    return pyproj.Transformer.from_crs("EPSG:4326", "EPSG:2056", always_xy=True).transform(lon, lat)


def multiline(*parts):
    """A multi-line in LV95 of `parts` in degrees, so the reader must project it back."""
    import shapely

    return shapely.to_wkb(shapely.multilinestrings([shapely.linestrings([lv95(*point) for point in part]) for part in parts]))


def column(values):
    """A column of `values`; an integer column with a null gets a mask, as a source file has it."""
    import numpy as np

    if None in values and all(type(value) is int for value in values if value is not None):
        return np.array([value or 0 for value in values], np.int64), np.array([value is None for value in values])
    return np.array(values, dtype=object if None in values else None), None


def write(path, layer, fields, rows):
    import numpy as np
    import pyogrio.raw

    geometry = np.array([row[0] for row in rows], dtype=object)
    columns, masks = zip(*(column([row[1 + i] for row in rows]) for i in range(len(fields))))
    pyogrio.raw.write(path, geometry, list(columns), fields, field_mask=list(masks), layer=layer, driver="GPKG",
                      crs="EPSG:2056", geometry_type="MultiLineString", append=Path(path).exists())


ROUTE_FIELDS = ["Nr", "Name", "Level", "Up", "Way"]
NO_STAGE = [(multiline([(9, 9), (9.1, 9.1)]), "0", 1, "", "", "out")]


def read(routes, stages=(), description=DESCRIPTION, route_fields=ROUTE_FIELDS):
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory) / "routes.gpkg"
        write(path, "Route", route_fields, routes)
        write(path, "Stage", ["Nr", "Stage", "From", "To", "Way"], stages or NO_STAGE)
        return sources.read({"id": "test", "credit": "Test", "description": description}, {"routes.gpkg": path}, BOUNDS)


class RouteSourceTest(unittest.TestCase):
    def test_routes_stages_and_keys_read_by_the_rules_of_the_common_form(self):
        # A straight middle point and a repeated point leave in the simplification.
        two_parts = multiline([(8.01, 47.01), (8.015, 47.01), (8.02, 47.01), (8.02, 47.01)], [(8.03, 47.02), (8.04, 47.03)])
        far = multiline([(9.0, 46.0), (9.1, 46.1)])
        stage = lambda x: multiline([(x, 47.05), (x + 0.01, 47.05)])
        got = read(
            [(two_parts, "561", "Rigi Panoramaweg", "local", 620, "both"),
             (far, "4", "Far", "local", 1, "both"),
             (None, "3", "Alpen", "national", None, "out"), (None, "3", "Alpen", "national", None, "back"),
             (multiline([(8.05, 47.05), (8.06, 47.06)]), "Lake loop", "Lake loop", "regional", None, "both")],
            [(stage(8.01), "3", 1, "A", "B", "out"), (stage(8.02), "3", 2, "B", "C", "out"), (stage(9.5), "3", 3, "C", "D", "out"),
             (stage(8.03), "3", 2, "C", "B", "back"), (stage(8.04), "999", 1, "X", "Y", "out")])
        text = int(hashlib.sha256(b"Lake loop").hexdigest(), 16) % 10**10
        routes = [
            {"key": 3, "kind": "hiking", "name": "Alpen", "rank": 3, "ref": "3", "stages": [
                {"members": [[8010000, 47050000, 8020000, 47050000]], "name": "Alpen, A – B", "stage": 1},
                {"members": [[8020000, 47050000, 8030000, 47050000]], "name": "Alpen, B – C", "stage": 2},
                {"members": [], "name": "Alpen, C – D", "stage": 3}]},
            {"ascent_m": 620, "key": 561, "kind": "hiking", "members": [
                [8010000, 47010000, 8020000, 47010000], [8030000, 47020000, 8040000, 47030000]],
             "name": "Rigi Panoramaweg", "rank": 1, "ref": "561"},
            {"key": text, "kind": "hiking", "members": [[8050000, 47050000, 8060000, 47060000]], "name": "Lake loop",
             "rank": 0, "ref": "Lake loop"}]
        self.assertEqual(got, {"credit": "Test", "id": "test", "origin": 7, "routes": sorted(routes, key=lambda route: route["key"])})

    def test_the_known_projection_of_the_fixture(self):
        self.assertEqual([round(value) for value in lv95(8.01, 47.01)], [2643448, 1206708])

    def test_an_integer_key_field_with_a_null_keeps_its_numbers(self):
        line = multiline([(8.01, 47.01), (8.02, 47.02)])
        got = read([(line, 3, "A", "local", None, "both"), (line, 4, "B", "local", None, "both"),
                    (line, None, "C", "local", None, "both")])
        self.assertEqual([route["key"] for route in got["routes"]], [3, 4])

    def test_a_piece_with_two_key_fields_is_a_member_of_both_routes_and_names_each(self):
        description = DESCRIPTION.replace('key = "Nr"', 'key = ["N1", "N2"]').replace('ref = "{Nr}"\nname = "{Name}"', 'name = "{key}"')
        line = lambda x: multiline([(x, 47.01), (x + 0.01, 47.01)])
        got = read([(line(8.01), "Altmuehl", "Jurasteig", "local", None, "both"),
                    (line(8.03), None, "Jurasteig", "local", None, "both")],
                   description=description, route_fields=["N1", "N2", "Level", "Up", "Way"])
        named = {route["name"]: len(route["members"]) for route in got["routes"]}
        self.assertEqual(named, {"Altmuehl": 1, "Jurasteig": 2})

    def test_two_texts_with_one_key_fail(self):
        line = multiline([(8.01, 47.01), (8.02, 47.02)])
        with mock.patch.object(sources, "text_key", return_value=5):
            with self.assertRaisesRegex(ValueError, "'Alpha' and 'Beta' give the one key 5"):
                read([(line, "Alpha", "A", "local", None, "both"), (line, "Beta", "B", "local", None, "both")])

    def test_a_description_with_an_unknown_key_fails(self):
        with self.assertRaisesRegex(ValueError, "`route` has the unknown keys colour"):
            sources.parse(DESCRIPTION.replace('layer = "Route"', 'layer = "Route"\ncolour = "red"'))


if __name__ == "__main__":
    unittest.main()
