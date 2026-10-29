import importlib.util
import io
import sys
import unittest
from contextlib import redirect_stdout
from pathlib import Path

MODULE_PATH = Path(__file__).parents[1] / "cairn.py"
SPEC = importlib.util.spec_from_file_location("cairn", MODULE_PATH)
cairn = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = cairn
SPEC.loader.exec_module(cairn)

CONFIG = {
    "runtimes": {"tools/": "host"},
    "shell_glue": {"max_lines": 100},
    "tooling": {"path": "tools/", "area": "Tooling"},
}


def doc(name, owns, kind="system", area="Device", depends_on=(), status="draft"):
    return cairn.Doc(name, kind, area, "", tuple(owns), tuple(depends_on), (), status)


def analyse(docs, files, units=(), edges=()):
    return cairn.analyse(docs, list(files), list(units), list(edges), {}, CONFIG)


def ids(result, rule):
    return sorted(v.id for v in result.violations if v.rule == rule)


def unit(name, entry):
    return {"name": name, "dir": entry.rsplit("/src/", 1)[0], "entry": entry}


class OwnershipTests(unittest.TestCase):
    def test_a_file_needs_exactly_one_owner(self):
        docs = [doc("a", ["lib/**"]), doc("b", ["lib/shared.rs"])]
        result = analyse(docs, ["lib/x.rs", "lib/shared.rs", "other/y.rs"])
        self.assertEqual(ids(result, "ownership"), ["owned twice lib/shared.rs", "unowned other/y.rs"])

    def test_a_system_beats_a_shell(self):
        docs = [doc("app", ["app/**"], kind="shell"), doc("nav", ["app/src/nav*"])]
        self.assertEqual(cairn.owners(["app/src/nav.rs", "app/src/main.rs"], docs),
                         {"app/src/nav.rs": ["nav"], "app/src/main.rs": ["app"]})

    def test_a_single_star_stays_in_its_folder(self):
        pattern = cairn.glob_regex(("tools/*.py",))
        self.assertTrue(pattern.fullmatch("tools/a.py"))
        self.assertFalse(pattern.fullmatch("tools/wiki/a.py"))


class DependencyTests(unittest.TestCase):
    UNITS = (unit("ca", "a/src/lib.rs"), unit("cb", "b/src/lib.rs"), unit("cc", "c/src/lib.rs"))

    def test_a_cycle_is_a_violation_even_when_declared(self):
        docs = [doc("a", ["a/**"], depends_on=["b"]), doc("b", ["b/**"], depends_on=["a"]), doc("c", ["c/**"])]
        edges = [("ca", "cb", False), ("cb", "ca", False), ("cc", "ca", False)]
        result = analyse(docs, ["a/src/lib.rs", "b/src/lib.rs", "c/src/lib.rs"], self.UNITS, edges)
        self.assertEqual(result.cycles, [["a", "b"]])
        self.assertEqual(ids(result, "cycle"), ["cycle a -> b", "cycle b -> a"])
        self.assertEqual(ids(result, "dependency"), ["dependency c -> a"])

    def test_a_test_edge_closes_no_cycle_and_needs_test_depends_on(self):
        docs = [doc("a", ["a/**"], depends_on=["b"]), doc("b", ["b/**"]), doc("c", ["c/**"])]
        edges = [("ca", "cb", False), ("cb", "ca", True)]
        result = analyse(docs, ["a/src/lib.rs", "b/src/lib.rs"], self.UNITS, edges)
        self.assertEqual(result.cycles, [])
        self.assertEqual(ids(result, "dependency"), ["test dependency b -> a"])


class RuleTests(unittest.TestCase):
    def test_r4_only_the_tooling_area_lives_in_tools(self):
        docs = [doc("tooling", ["tools/ci.py"], area="Tooling"), doc("maps", ["tools/maps.py", "tools/tiles.py"])]
        result = analyse(docs, ["tools/ci.py", "tools/maps.py", "tools/tiles.py"])
        [violation] = [v for v in result.violations if v.rule == "R4"]
        self.assertEqual((violation.id, violation.size), ("R4 maps", 2))


class RatchetTests(unittest.TestCase):
    def failing(self, violations, baseline, approved):
        with redirect_stdout(io.StringIO()):
            return cairn.report(cairn.Result(violations), baseline, [], approved)

    def test_a_new_violation_fails_only_for_an_approved_system(self):
        new = cairn.Violation("R4", "R4 maps", frozenset({"maps"}), "", 3)
        self.assertEqual(self.failing([new], {}, set()), [])
        self.assertEqual(self.failing([new], {}, {"maps"}), [new])
        self.assertEqual(self.failing([new], {"R4 maps": 3}, {"maps"}), [])
        self.assertEqual(self.failing([new], {"R4 maps": 2}, {"maps"}), [new])

    def test_update_never_grows_the_baseline(self):
        violations = [
            cairn.Violation("R4", "R4 maps", frozenset({"maps"}), "", 5),
            cairn.Violation("R4", "R4 tiles", frozenset({"tiles"}), "", 1),
            cairn.Violation("R3", "R3 app", frozenset({"app"}), "", 90),
        ]
        baseline = {"R4 maps": 3, "R3 app": 120, "R1 gone": 1}
        self.assertEqual(cairn.updated(violations, baseline), {"R3 app": 90, "R4 maps": 3})


if __name__ == "__main__":
    unittest.main()
