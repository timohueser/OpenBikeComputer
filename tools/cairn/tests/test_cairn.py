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

from adapters import cargo  # noqa: E402 — cairn.py puts its own folder on the path

CONFIG = {
    "runtimes": {"firmware/": "device", "host/": "host", "tools/": "host"},
    "tooling": {"path": "tools/", "area": "Tooling"},
}


def doc(name, owns, kind="system", area="Device", depends_on=(), status="draft"):
    return cairn.Doc(name, kind, area if kind == "system" else "", "", tuple(owns), tuple(depends_on), (), status)


def analyse(docs, files, units=(), edges=(), lines=None):
    return cairn.analyse(docs, list(files), list(units), list(edges), lines or {}, CONFIG)


def ids(result, rule):
    return sorted(v.id for v in result.violations if v.rule == rule)


def sizes(result, rule):
    return {v.id: v.size for v in result.violations if v.rule == rule}


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


class CargoAdapterTests(unittest.TestCase):
    def package(self, name, dependencies=(), targets=None):
        return {
            "id": name,
            "name": name,
            "manifest_path": f"/repo/{name}/Cargo.toml",
            "targets": targets or [{"kind": ["lib"], "src_path": f"/repo/{name}/src/lib.rs"}],
            "dependencies": [
                {"name": target, "kind": kind, **({"path": f"/repo/{target}"} if path else {})}
                for target, kind, path in dependencies
            ],
        }

    def test_a_production_edge_hides_a_test_edge_and_the_main_binary_is_the_entry(self):
        tool = self.package("tool", targets=[
            {"kind": ["bin"], "src_path": "/repo/tool/src/bin/extra.rs"},
            {"kind": ["bin"], "src_path": "/repo/tool/src/main.rs"},
        ])
        a = self.package("a", [("b", "dev", True), ("b", None, True), ("tool", "dev", True), ("serde", None, False)])
        packages = [a, self.package("b"), tool]
        units, edges = cargo.collect(Path("/repo"), [{"packages": packages, "workspace_members": ["a", "b", "tool"]}])
        self.assertEqual({u["name"]: u["entry"] for u in units},
                         {"a": "a/src/lib.rs", "b": "b/src/lib.rs", "tool": "tool/src/main.rs"})
        self.assertEqual(edges, [("a", "b", False), ("a", "tool", True)])


class RuleTests(unittest.TestCase):
    def test_r1_counts_the_files_of_other_systems_in_a_crate(self):
        docs = [doc("runtime", ["app/src/lib.rs"]), doc("nav", ["app/src/nav*"])]
        crate = [unit("app", "app/src/lib.rs")]
        result = analyse(docs, ["app/src/lib.rs", "app/src/nav.rs"], crate)
        self.assertEqual(sizes(result, "R1"), {"R1 app nav": 1})
        grown = analyse(docs, ["app/src/lib.rs", "app/src/nav.rs", "app/src/nav_map.rs"], crate)
        self.assertEqual(cairn.new_violations(grown.violations, {"R1 app nav": 1})[0].size, 2)

    def test_r2_one_root_for_each_runtime(self):
        docs = [doc("maps", ["host/bake/**", "tools/bake.py", "firmware/read/**"])]
        result = analyse(docs, ["host/bake/lib.rs", "tools/bake.py", "firmware/read/lib.rs"])
        self.assertEqual(ids(result, "R2"), ["R2 maps host host/", "R2 maps host tools/"])

    def test_r3_records_every_shell_with_code_of_its_own(self):
        docs = [doc("app", ["app/**"], kind="shell"), doc("web", ["web/**"], kind="shell"), doc("nav", ["app/src/nav.rs"])]
        lines = {"app/src/main.rs": 40, "app/src/nav.rs": 500, "web/page.ts": 0}
        result = analyse(docs, lines, lines=lines)
        self.assertEqual(sizes(result, "R3"), {"R3 app": 40})

    def test_r4_only_the_tooling_area_lives_in_tools(self):
        docs = [doc("tooling", ["tools/ci.py"], area="Tooling"), doc("maps", ["tools/maps.py", "tools/tiles.py"])]
        result = analyse(docs, ["tools/ci.py", "tools/maps.py", "tools/tiles.py"])
        self.assertEqual(sizes(result, "R4"), {"R4 maps": 2})


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

    def test_an_unowned_file_fails_once_any_system_is_approved(self):
        unowned = cairn.Violation("ownership", "unowned x.rs", frozenset(), "")
        self.assertEqual(self.failing([unowned], {}, set()), [])
        self.assertEqual(self.failing([unowned], {}, {"maps"}), [unowned])

    def test_update_never_grows_the_baseline(self):
        violations = [
            cairn.Violation("R4", "R4 maps", frozenset({"maps"}), "", 5),
            cairn.Violation("R4", "R4 tiles", frozenset({"tiles"}), "", 1),
            cairn.Violation("R3", "R3 app", frozenset({"app"}), "", 90),
        ]
        baseline = {"R4 maps": 3, "R3 app": 120, "R1 gone": 1}
        self.assertEqual(cairn.updated(violations, baseline), {"R3 app": 90, "R4 maps": 3})

    def test_a_baseline_that_grew_by_hand_is_caught(self):
        current = [cairn.Violation("R3", "R3 app", frozenset({"app"}), "", 90)]
        base = {"R3 app": 80}
        self.assertEqual(cairn.grown({"R3 app": 80}, base, current), [])
        self.assertEqual(cairn.grown({"R3 app": 90}, base, current), ["R3 app: 90"])
        self.assertEqual(cairn.grown({"R3 app": 80, "R1 new": 1}, base, current), ["R1 new: 1"])
        # With no baseline on the base, only the first baseline that `--accept` writes passes.
        self.assertEqual(cairn.grown({"R3 app": 90}, None, current), [])
        self.assertEqual(cairn.grown({"R3 app": 95}, None, current), ["R3 app: 95"])


if __name__ == "__main__":
    unittest.main()
