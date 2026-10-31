import importlib.util
import io
import json
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

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


class MapTests(unittest.TestCase):
    def test_the_figure_shows_the_tangles_and_drops_shells_and_implied_edges(self):
        docs = [doc("a", ["a/**"], depends_on=["b"]), doc("b", ["b/**"], depends_on=["a"]),
                doc("c", ["c/**"], area="Apps", depends_on=["e"]), doc("e", ["e/**"], depends_on=["a", "f"]),
                doc("f", ["f/**"], depends_on=["a"]), doc("s", ["s/**"], kind="shell", depends_on=["a"])]
        units = [unit(f"c{name}", f"{name}/src/lib.rs") for name in "abcefs"]
        edges = [("ca", "cb", False), ("cb", "ca", False), ("cc", "ca", False),
                 ("ce", "ca", False), ("ce", "cf", False), ("cf", "ca", False), ("cs", "ca", False)]
        result = analyse(docs, [u["entry"] for u in units], units, edges)
        page = cairn.system_map(docs, result).splitlines()
        self.assertEqual(page[page.index('  subgraph area0["Apps"]') + 1], '    s_c["c"]')
        self.assertEqual(page[page.index("  s_a ==> s_b"):page.index("```", page.index("  s_a ==> s_b"))],
                         ["  s_a ==> s_b", "  s_b ==> s_a", "  s_c ==> s_a", "  s_c -.-> s_e", "  s_e --> s_f", "  s_f --> s_a"])
        self.assertIn("| [a](a.md) |  | draft | b | b, c, e, f, s |", page)
        self.assertIn("### Shells", page)
        tangles = page[page.index("## Tangles") + 2:-2]
        self.assertEqual(tangles, ["- A cycle between a, b: a -> b, b -> a", "- c -> a is not in the doc of c"])
        self.assertNotIn("### Shells", cairn.system_map(docs[:-1], analyse(docs[:-1], [], units[:-1], edges[:-1])))

    def test_map_data_gives_each_system_its_lines_and_problems(self):
        docs = [doc("a", ["a/**", "tools/a.py"], depends_on=["b"]), doc("b", ["b/**"], kind="shell")]
        units = [unit("ca", "a/src/lib.rs"), unit("cb", "b/src/lib.rs")]
        files = [u["entry"] for u in units] + ["tools/a.py"]
        result = analyse(docs, files, units, [("ca", "cb", False)])
        data = json.loads(cairn.map_data(docs, files, {"a/src/lib.rs": 10, "tools/a.py": 5, "b/src/lib.rs": 7}, result))
        a, b = data["systems"]
        self.assertEqual((a["name"], a["lines"], a["area"], a["depends_on"]), ("a", 15, "Device", ["b"]))
        self.assertEqual((b["lines"], b["area"]), (7, "Shells"))
        self.assertEqual(a["violations"], len(a["problems"]))
        self.assertIn("R4", {p["rule"] for p in a["problems"]})
        self.assertEqual(data["edges"], [{"from": "a", "to": "b", "test": False, "undeclared": False}])
        self.assertEqual(data["cycles"], [])

    def test_check_fails_on_a_stale_map_and_map_keeps_the_owners_text(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / ".cairn").mkdir()
            (root / cairn.CONFIG).write_text(json.dumps({**CONFIG, "docs": "docs/design", "baseline": ".cairn/baseline.json"}))
            (root / "docs/design").mkdir(parents=True)
            page = root / "docs/design" / cairn.MAP
            page.write_text(f"# Map\n\nMine.\n\n{cairn.MAP_BEGIN}\nold\n{cairn.MAP_END}\n")
            docs = [doc("routing", ["route/**"])]
            with mock.patch.object(cairn, "ROOT", root), \
                    mock.patch.object(cairn, "gather", return_value=(docs, [], [], [], {})), \
                    redirect_stdout(io.StringIO()) as out:
                self.assertEqual(cairn.main(["check"]), 1)
                self.assertIn("run `obc cairn map`", out.getvalue())
                self.assertEqual(cairn.main(["map"]), 0)
                self.assertEqual(cairn.main(["check"]), 0)
                data = page.with_name(cairn.MAP_DATA)
                self.assertEqual(json.loads(data.read_text())["systems"][0]["name"], "routing")
                written = json.loads(data.read_text())
                written["systems"][0].update(lines=99, violations=9, problems=[{"rule": "R3", "message": "9 lines"}])
                data.write_text(json.dumps(written))
                self.assertEqual(cairn.main(["check"]), 0)
                written["systems"][0]["purpose"] = "Old."
                data.write_text(json.dumps(written))
                self.assertEqual(cairn.main(["check"]), 1)
                self.assertIn("map.json is out of date", out.getvalue())
            self.assertTrue(page.read_text().startswith("# Map\n\nMine.\n\n"))
            self.assertIn("| [routing](routing.md) |", page.read_text())
        block = f"{cairn.MAP_BEGIN}\n{cairn.MAP_END}"
        for broken in ("# Map\n", cairn.MAP_BEGIN, block + cairn.MAP_END, cairn.MAP_BEGIN + block, cairn.MAP_END + cairn.MAP_BEGIN):
            with self.assertRaisesRegex(cairn.CairnError, "^docs/design/README.md: ", msg=broken):
                cairn.with_map(broken, block, "docs/design/README.md")


class LeafTests(unittest.TestCase):
    def test_leaves_keep_modules_whole_stay_in_one_crate_and_list_the_crate_uses(self):
        docs = [doc("a", ["a/**", "a2/**", "tools/a.py"]), doc("b", ["b/**"]), doc("c", ["c/**"])]
        lines = {"a/src/big.rs": 2000, "a/src/lib.rs": 100, "a/src/mod_x.rs": 150, "a/src/mod_x/inner.rs": 100,
                 "a/src/mod_x/more.rs": 100, "a/src/small.rs": 10, "a/tests/t.rs": 30, "a/vendor/v.rs": 5,
                 "tools/a.py": 20, "a2/src/lib.rs": 1, "b/src/lib.rs": 1, "c/src/lib.rs": 1}
        units = [unit(name, f"{folder}/src/lib.rs") for name, folder in (("ra", "a"), ("ra2", "a2"), ("rb", "b"), ("rc", "c"))]
        edges = [("ra", "ra2", False), ("ra", "rb", False), ("ra", "rc", True), ("rb", "rc", False)]
        found = cairn.leaves("a", docs, list(lines), units, edges, lines, 200, 60, ["**/tests/**", "**/vendor/**"])
        self.assertEqual([(leaf.level, leaf.crate, leaf.files) for leaf in found], [
            ("", None, ("tools/a.py",)),
            ("a/src/", "ra", ("a/src/big.rs",)),
            ("a/src/", "ra", ("a/src/lib.rs",)),
            # A module file and its folder that are too large together are cut at the folder.
            ("a/src/mod_x/", "ra", ("a/src/mod_x.rs",)),
            ("a/src/mod_x/", "ra", ("a/src/mod_x/inner.rs", "a/src/mod_x/more.rs")),
            ("a/src/", "ra", ("a/src/small.rs",)),
            ("a2/", "ra2", ("a2/src/lib.rs",)),
        ])
        self.assertEqual((found[4].lines, found[4].uses, found[4].test_uses), (200, ("ra2 (a)", "rb (b)"), ("rc (c)",)))
        small = {f"m/f{i}.rs": 1 for i in range(5)}
        self.assertEqual(cairn.cut(sorted(small), small, 200, 2, "m/"),
                         [("m/", ["m/f0.rs", "m/f1.rs"]), ("m/", ["m/f2.rs", "m/f3.rs"]), ("m/", ["m/f4.rs"])])


class SkillBudgetTests(unittest.TestCase):
    def test_check_fails_when_a_skill_file_is_over_its_token_budget(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            skill = root / "skill"
            (skill / "reference").mkdir(parents=True)
            (skill / "SKILL.md").write_text("r" * 40)
            (skill / "reference/doc.md").write_text("d" * 44)
            (root / ".cairn").mkdir()
            config = {**CONFIG, "docs": "docs/design", "baseline": ".cairn/baseline.json",
                      "skill": {"path": "skill", "router_tokens": 10, "reference_tokens": 10}}
            (root / cairn.CONFIG).write_text(json.dumps(config))
            (root / "docs/design").mkdir(parents=True)
            (root / "docs/design" / cairn.MAP).write_text(f"{cairn.MAP_BEGIN}\n{cairn.MAP_END}\n")
            with mock.patch.object(cairn, "ROOT", root), \
                    mock.patch.object(cairn, "gather", return_value=([], [], [], [], {})), \
                    redirect_stdout(io.StringIO()) as out:
                self.assertEqual(cairn.main(["map"]), 0)
                self.assertEqual(cairn.main(["check"]), 1)
                self.assertIn("     SKILL.md: 10 of 10\n  FAIL reference/doc.md: 11 of 10\n", out.getvalue())
                (skill / "reference/doc.md").write_text("d" * 43)
                self.assertEqual(cairn.main(["check"]), 0)


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
        self.assertEqual(sorted(v.message for v in result.violations if v.rule == "R2"),
                         ["maps has host code in host/, and also in tools/", "maps has host code in tools/, and also in host/"])

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
            docs = [doc("maps", []), doc("app", [], kind="shell")]
            return cairn.report(cairn.Result(violations), baseline, docs, approved)

    def test_a_new_violation_fails_only_for_an_approved_system(self):
        new = cairn.Violation("R4", "R4 maps", frozenset({"maps"}), "maps in tools", 3)
        self.assertEqual(self.failing([new], {}, set()), [])
        self.assertEqual(self.failing([new], {}, {"maps"}), ["maps in tools"])
        self.assertEqual(self.failing([new], {"R4 maps": 3}, {"maps"}), [])
        self.assertEqual(self.failing([new], {"R4 maps": 2}, {"maps"}), ["maps in tools"])

    def test_an_unowned_file_fails_once_any_system_is_approved(self):
        unowned = cairn.Violation("ownership", "unowned x.rs", frozenset(), "x.rs has no owner")
        self.assertEqual(self.failing([unowned], {}, set()), [])
        self.assertEqual(self.failing([unowned], {}, {"maps"}), ["x.rs has no owner"])

    def test_a_shrunk_or_gone_entry_fails_only_for_an_approved_system(self):
        shrunk = cairn.Violation("R4", "R4 maps", frozenset({"maps"}), "", 2)
        baseline = {"R4 maps": 3, "R3 app": 40}
        self.assertEqual(self.failing([shrunk], baseline, set()), [])
        self.assertEqual(self.failing([shrunk], baseline, {"maps"}), ["R4 maps"])
        self.assertEqual(self.failing([shrunk], baseline, {"app"}), ["R3 app"])

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


    def test_the_base_baseline_path_comes_from_the_base_config(self):
        files = {
            ".cairn/config.json": '{"baseline": "old/baseline.json"}',
            "old/baseline.json": '{"R3 app": 80}',
        }
        self.assertEqual(cairn.base_baseline(files.get), {"R3 app": 80})
        self.assertIsNone(cairn.base_baseline({}.get))


class BaseApprovalTests(unittest.TestCase):
    def test_a_doc_set_back_to_draft_stays_approved_against_the_base(self):
        def routing(status):
            return f"---\nsystem: routing\nkind: system\narea: Device\npurpose: P.\nowns: [route/**]\nstatus: {status}\n---\n"

        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)

            def git(*args):
                return subprocess.run(["git", "-C", folder, "-c", "user.name=t", "-c", "user.email=t@t", *args],
                                      check=True, capture_output=True, text=True).stdout.strip()

            def commit(files):
                for path, text in files.items():
                    (root / path).parent.mkdir(parents=True, exist_ok=True)
                    (root / path).write_text(text)
                git("add", "-A")
                self.assertEqual(cairn.main(["map"]), 0)
                git("add", "-A")
                git("commit", "--quiet", "-m", "c")
                return git("rev-parse", "HEAD")

            git("init", "--quiet")
            config = {**CONFIG, "docs": "docs/design", "baseline": ".cairn/baseline.json", "adapters": {},
                      "sources": {"extensions": [".rs"], "files": []}}
            with mock.patch.object(cairn, "ROOT", root), redirect_stdout(io.StringIO()) as out:
                base = commit({".cairn/config.json": json.dumps(config), "docs/design/routing.md": routing("approved"),
                               "docs/design/README.md": f"{cairn.MAP_BEGIN}\n{cairn.MAP_END}\n", "route/a.rs": "a\n"})
                commit({"docs/design/routing.md": routing("draft"), "other/b.rs": "b\n"})
                self.assertEqual(cairn.main(["check"]), 0)
                self.assertEqual(cairn.main(["check", "--base", base]), 1, out.getvalue())
        self.assertIn("FAIL other/b.rs has no owner", out.getvalue())


if __name__ == "__main__":
    unittest.main()
