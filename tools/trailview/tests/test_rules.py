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
    ("materials", ["materials", 9, "clumps"], 2, "materials.json: materials[9].clumps: 2 is not a number from 0 to 1"),
    ("styles/trullo", ["roof"], "dome", 'styles/trullo.json: roof: "dome" is not one of "cone", "flat"'),
    # Not the prototype's: the engine draws each model that a pack names, also one that a child pack inherits.
    ("packs/alps", ["trees", "norway-spruce", "model"], "no-such-tree", "regions/alps.insubric.json: pack alps.insubric: tree norway-spruce has no model no-such-tree"),
    ("packs/alps", ["ground", "bilberry", "model"], "norway-spruce", "regions/alps.json: pack alps: ground plant bilberry has no model norway-spruce"),
    ("packs/alps", ["orchard"], "no-such-orchard", "regions/alps.json: pack alps: orchard has no model no-such-orchard"),
    ("packs/alps", ["style", "xx-yy"], "chalet", "regions/alps.json: xx-yy is not a place of countries.json"),
    ("packs/alps", ["ground", "bilberry", "on"], ["#heath"],
     'packs/alps.json: ground.bilberry.on[0]: "#heath" is not a cover, a material or a #tag of materials.json'),
    ("regions/fjell", ["look", "patches", "scrub", "size_m"], 4,
     "regions/fjell.json: look.patches.scrub.size_m: 4 is above 3: the dots of larger bushes stop where the far nodes start"),
    ("regions/fjell", ["look", "patches", "heather", "size_m"], 0.5,
     "regions/fjell.json: look.patches.heather.size_m: 0.5 is below 0.74: finer blotches shimmer before the far nodes start"),
    ("regions/fjell", ["look", "patches", "lichen"], {"on": ["moss"], "colour": "#9a9a80", "share": 0.3, "size_m": 2, "form": "blotches"},
     "regions/fjell.json: look.patches: 3 patches, more than the 2 that the ground draws"),
    ("regions/italy-med", ["look", "patches", "heather"], {"colour": "#6e4f63"}, "regions/italy-med.json: look.patches.heather: has no on, share, size_m, form"),
    ("materials", ["materials"], 65 * [{"id": "none", "cover": "grass"}],
     "materials.json: 65 materials, more than the 64 that a code of the ground tiles holds"),
    # Not the prototype's: a ground source is a row of data/sources.toml, and its legend names the vocabulary.
    ("ground", ["material", "sources", 3, "source"], "land-cover-xx",
     'ground.json: material.sources[3].source: "land-cover-xx" is not a source of data/sources.toml'),
    ("ground", ["lithology", "sources", 0, "legend", "classes", 0, 1], "lava",
     'ground.json: lithology.sources[0].legend.classes[0][1]: "lava" is not one of ' + ", ".join(
         f'"{entry["id"]}"' for entry in RULESET["materials"]["lithologies"])),
    ("ground", ["lithology", "sources", 0, "zoom"], DELETE, "ground.json: lithology.sources[0]: has no zoom"),
    ("ground", ["material", "sources", 3, "zoom"], 11, "ground.json: material.sources[3].zoom: only a vector lithology source has a zoom"),
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
    def test_the_ruleset_has_no_problems(self):
        self.assertEqual(rules.validate(RULESET), [])

    def test_each_bad_case_of_the_prototype_check_is_a_problem(self):
        for file, path, value, problem in CASES:
            with self.subTest(file=file, path=path):
                self.assertIn(problem, rules.validate(edited(file, path, value)))

    def test_more_region_ids_than_one_byte_of_the_region_tiles_holds_is_a_problem(self):
        ruleset = copy.deepcopy(RULESET)
        world = ruleset["regions"]["world"]
        ruleset["regions"].update({f"world.extra-{i}": {**world, "extends": "world", "where": [[0, 0, 1, 1]]}
                                   for i in range(255 - len(rules.region_ids(ruleset)))})
        self.assertEqual([problem for problem in rules.validate(ruleset) if problem.startswith("regions/: ")], [])
        ruleset["regions"]["world.extra-last"] = ruleset["regions"]["world.extra-0"]
        self.assertIn("regions/: 256 region ids, more than the 255 that one byte of the region tiles holds", rules.validate(ruleset))

    def test_a_pack_with_more_tree_species_than_the_forest_holds_or_an_unknown_model_is_a_problem(self):
        ruleset = copy.deepcopy(RULESET)
        trees = ruleset["packs"]["alps"]["trees"]
        spruce = {**trees["norway-spruce"], "model": "norway-spruce"}
        trees.update({f"spruce-{i}": spruce for i in range(rules.MAX_TREE_SPECIES)})
        trees["no-such-tree"] = {**spruce, "model": "no-such-tree"}
        problems = rules.validate(ruleset)
        self.assertTrue(any(p.startswith("regions/alps.json: pack alps has ") and p.endswith("more than the 32 that the forest holds") for p in problems), problems)
        self.assertIn("regions/alps.json: pack alps: tree no-such-tree has no model no-such-tree", problems)

    def test_more_house_styles_than_the_style_table_holds_is_a_problem(self):
        ruleset = copy.deepcopy(RULESET)
        ruleset["styles"].update({f"cottage-{i}": ruleset["styles"]["cottage"] for i in range(rules.MAX_STYLES - len(ruleset["styles"]))})
        self.assertEqual([p for p in rules.validate(ruleset) if p.startswith("styles/: ")], [])
        ruleset["styles"]["cottage-last"] = ruleset["styles"]["cottage"]
        self.assertIn("styles/: 64 house styles, more than the 63 that the engine's style table holds", rules.validate(ruleset))

    def test_a_pack_with_more_shrubs_or_ground_plants_than_the_plants_hold_is_a_problem(self):
        ruleset = copy.deepcopy(RULESET)
        pack = ruleset["packs"]["alps"]
        shrub = next(rule for rule in pack["trees"].values() if rule.get("cover") == "shrub")
        pack["trees"].update({f"shrub-{i}": shrub for i in range(rules.MAX_PLANT_SLOTS)})
        pack["ground"].update({f"grass-{i}": {"model": "grass"} for i in range(rules.MAX_PLANT_SLOTS)})
        problems = rules.validate(ruleset)
        for kind in ("shrubs", "ground plants"):
            self.assertTrue(any(p.startswith("regions/alps.json: pack alps has ") and p.endswith(f"{kind}, more than the 16 that the plants hold") for p in problems), problems)

    def test_a_number_that_json_does_not_have_is_a_problem(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "trailview"
            shutil.copytree(rules.RULESET, root)
            path = root / "packs/aegean.json"
            path.write_text(path.read_text().replace('"weight": 1', '"weight": Infinity', 1))
            with self.assertRaisesRegex(ValueError, "packs/aegean.json: Infinity is not JSON"):
                rules.load(root)

    def test_a_region_has_an_id_for_itself_and_for_each_place_that_its_chain_names_and_the_chains_stay_whole(self):
        bundled, regions = rules.bundle(RULESET)
        ids = bundled["ids"]
        self.assertEqual(regions["ids"], ids)
        self.assertEqual(sorted({name for name, _ in ids}), sorted(RULESET["regions"]))
        self.assertEqual([place for name, place in ids if name == "celtic"], ["*", "ie"])
        self.assertEqual([place for name, place in ids if name == "celtic.irish-coast"], ["*"], "its pack replaces the map with one style")
        self.assertEqual([place for name, place in ids if name == "world"], ["*"])
        insubric = bundled["regions"]["alps.insubric"]
        self.assertEqual(insubric["extends"], "alps")
        self.assertNotIn("where", insubric)
        self.assertIn("where", regions["regions"]["alps.insubric"])
        self.assertEqual({e["short"] for e in regions["ecoregions"]} - set(bundled["regions"]), set())


if __name__ == "__main__":
    unittest.main()
