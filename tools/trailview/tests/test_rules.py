"""The ruleset check rejects what the prototype's `tune/check.mjs` rejects, with its words, and the
bundle gives each region its id."""

import copy
import shutil
import tempfile
import unittest
from pathlib import Path

from tools.trailview import rules

RULESET = rules.load()
DELETE = object()
# The messages are the ones that the prototype's check prints for the same edit. Each value breaks the
# engine or the region tiles.
CASES = [
    ("materials", ["materials", 3, "cover"], "lava",
     "materials.json: materials[3].cover: lava is not one of tree, shrub, grass, crop, built, bare, snow, water, wetland, mangrove, moss"),
    ("materials", ["materials", 4, "id"], "sea", "materials.json: materials[4].id: sea is also materials[1]"),
    ("regions/alps", ["extends"], "nowhere", "regions/alps.json: extends: no regions/nowhere.json"),
    ("regions/alps.insubric", ["where"], DELETE, "regions/alps.insubric.json: where: a subregion needs its areas"),
    ("regions/world", ["extends"], "alps.insubric", "regions/world.json: extends: a loop world › alps.insubric › alps › world"),
    ("regions/alps", ["look", "roadLine"], {"ch": "#ffffff"}, 'regions/alps.json: look.roadLine: has no "*" entry'),
    ("regions/alps.insubric", ["where"], [[[8, 46], [9, 46]]],
     "regions/alps.insubric.json: where[0]: [[8,46],[9,46]] is not a box [west, south, east, north] or a ring of 3 or more [lon, lat]"),
    ("regions/alps", ["look", "ramps"], {"grass": ["#fff", "#000000", "#000000"]},
     'regions/alps.json: look.ramps.grass[0]: "#fff" is not a #rrggbb colour'),
    ("regions/alps", ["look", "bare_luma"], [0.1, 0.2], "regions/alps.json: look.bare_luma: [0.1,0.2] is not a list of 4"),
    ("regions/world", ["leaf"], "deciduous", 'regions/world.json: leaf: "deciduous" is not one of "needleleaved", "broadleaved", "mixed"'),
    ("overlays", ["atlantis"], {"look": {}}, "overlays/atlantis.json: has no regions/atlantis.json"),
    ("overlays", ["alps"], {"look": {"banks": "#000000"}}, "overlays/alps.json: look.banks: is an unknown key"),
    # Not the prototype's: the engine draws each model that a pack names, also one that a child pack inherits.
    ("packs/alps", ["trees", "norway-spruce", "model"], "monkey-puzzle", "packs/alps.insubric.json: trees.norway-spruce: the engine has no model monkey-puzzle"),
    ("packs/alps", ["ground", "bilberry", "model"], "norway-spruce", "packs/alps.json: ground.bilberry: the engine has no plant norway-spruce"),
]


def edited(file, path, value):
    ruleset = copy.deepcopy(RULESET)
    folder, _, name = file.rpartition("/")
    data = ruleset[folder][name] if folder else ruleset[file]
    *head, last = path
    for key in head:
        data = data[key]
    if value is DELETE:
        del data[last]
    else:
        data[last] = value
    return ruleset


class RulesetTest(unittest.TestCase):
    def test_the_snapshot_has_no_problems(self):
        self.assertEqual(rules.validate(RULESET), [])

    def test_each_bad_case_of_the_prototype_check_is_a_problem(self):
        for file, path, value, problem in CASES:
            with self.subTest(file=file, path=path):
                self.assertIn(problem, rules.validate(edited(file, path, value)))

    def test_more_regions_than_one_byte_of_the_region_tiles_holds_is_a_problem(self):
        ruleset = copy.deepcopy(RULESET)
        world = ruleset["regions"]["world"]
        ruleset["regions"].update({f"world.extra-{i}": {**world, "extends": "world", "where": [[0, 0, 1, 1]]} for i in range(255 - len(ruleset["regions"]))})
        self.assertEqual([problem for problem in rules.validate(ruleset) if problem.startswith("regions/: ")], [])
        ruleset["regions"]["world.extra-last"] = ruleset["regions"]["world.extra-0"]
        self.assertIn("regions/: 256 regions, more than the 255 that one byte of the region tiles holds", rules.validate(ruleset))

    def test_a_number_that_json_does_not_have_is_a_problem(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "trailview"
            shutil.copytree(rules.RULESET, root)
            path = root / "packs/aegean.json"
            path.write_text(path.read_text().replace('"weight": 1', '"weight": Infinity', 1))
            with self.assertRaisesRegex(ValueError, "packs/aegean.json: Infinity is not JSON"):
                rules.load(root)

    def test_a_region_id_is_its_index_among_the_sorted_regions_and_the_chains_stay_whole(self):
        bundled, regions = rules.bundle(RULESET)
        names = [region["id"] for region in bundled["regions"]]
        self.assertEqual(names, sorted(RULESET["regions"]))
        self.assertEqual([region["id"] for region in regions["regions"]], names)
        alps = bundled["regions"][names.index("alps.insubric")]
        self.assertEqual(alps["extends"], "alps")
        self.assertNotIn("where", alps)
        self.assertIn("where", regions["regions"][names.index("alps.insubric")])
        self.assertEqual({e["short"] for e in regions["ecoregions"]} - set(names), set())


if __name__ == "__main__":
    unittest.main()
