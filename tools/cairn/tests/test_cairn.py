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

from adapters import cargo, imports  # noqa: E402 — cairn.py puts its own folder on the path

CONFIG = {"tooling": {"path": "tools/", "area": "Tooling"}}


def doc(name, owns, kind="system", area="Device", depends_on=(), status="draft"):
    return cairn.Doc(name, kind, area if kind == "system" else "", "", tuple(owns), tuple(depends_on), (), status)


def analyse(docs, files, units=(), edges=()):
    return cairn.analyse(docs, list(files), list(units), list(edges), CONFIG)


def ids(result, rule):
    return sorted(v.id for v in result.violations if v.rule == rule)


def messages(result, rule):
    return [v.message for v in result.violations if v.rule == rule]


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


class RulesTableTests(unittest.TestCase):
    def test_a_name_in_the_check_column_must_be_in_a_file_that_its_cell_links(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / "docs/design").mkdir(parents=True)
            (root / "src").mkdir()
            (root / "src/a.rs").write_text("fn a_frame_keeps_its_size() {}\n")
            (root / "docs/design/nav.md").write_text(
                "## Rules that must hold\n\n| Rule | Check |\n| --- | --- |\n"
                "| `some_function` keeps the size. | `a_frame_keeps_its_size` in [`a.rs`](../../src/a.rs) |\n"
                "| The order holds. | `a_renamed_test` in [`a.rs`](../../src/a.rs); `obc shot --check` |\n"
                "| No link. | `a_test_with_no_link` |\n\n"
                "## Decisions\n\n`not_a_rule` is outside the table.\n")
            found = cairn.lost_names(root, "docs/design")
        self.assertEqual([line.split("`")[1] for line in found], ["a_renamed_test", "a_test_with_no_link"])


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


class ImportAdapterTests(unittest.TestCase):
    def test_a_relative_import_is_an_edge_between_two_tracked_files(self):
        texts = {
            "web/src/plan/app.ts": 'import { a } from "../view/engine.js";\nimport Map from "./Map.svelte";\n'
                                   'import "leaflet";\nimport x from "$lib/x";\nconst p = await import("../view");\n'
                                   'new Worker(new URL("./plan.worker.ts", import.meta.url));\nimport gone from "./gone";\n',
            "web/src/view/engine.ts": 'export * from "./store.svelte";\n',
        }
        tracked = {*texts, "web/src/plan/Map.svelte", "web/src/plan/plan.worker.ts", "web/src/view/index.ts",
                   "web/src/view/store.svelte.ts"}
        self.assertEqual(imports.collect(texts, tracked), [
            ("web/src/plan/app.ts", "web/src/plan/Map.svelte", False),
            ("web/src/plan/app.ts", "web/src/plan/plan.worker.ts", False),
            ("web/src/plan/app.ts", "web/src/view/engine.ts", False),
            ("web/src/plan/app.ts", "web/src/view/index.ts", False),
            ("web/src/view/engine.ts", "web/src/view/store.svelte.ts", False),
        ])

    def test_a_file_edge_counts_for_the_owners_of_the_two_files_and_a_test_file_gives_a_test_edge(self):
        docs = [doc("plan", ["web/plan/**"], depends_on=["view"]), doc("view", ["web/view/**"])]
        files = ["web/plan/app.ts", "web/plan/app.test.ts", "web/view/engine.ts"]
        edges = [("web/plan/app.ts", "web/view/engine.ts", False), ("web/view/engine.ts", "web/plan/app.ts", False),
                 ("web/plan/app.test.ts", "web/view/engine.ts", False)]
        result = cairn.analyse(docs, files, [], edges, {**CONFIG, "tests": ["*.test.ts"]})
        self.assertEqual(ids(result, "cycle"), ["cycle plan -> view", "cycle view -> plan"])
        only_test = cairn.analyse(docs, files, [], edges[2:], {**CONFIG, "tests": ["*.test.ts"]})
        self.assertEqual(only_test.edges, {("plan", "view", True)})


class RuleTests(unittest.TestCase):
    def test_r1_counts_the_files_of_other_systems_in_a_crate(self):
        docs = [doc("runtime", ["app/src/lib.rs"]), doc("nav", ["app/src/nav*"])]
        crate = [unit("app", "app/src/lib.rs")]
        result = analyse(docs, ["app/src/lib.rs", "app/src/nav.rs"], crate)
        self.assertEqual(ids(result, "R1"), ["R1 app nav"])
        more = analyse(docs, ["app/src/lib.rs", "app/src/nav.rs", "app/src/nav_map.rs"], crate)
        self.assertEqual(messages(more, "R1"), ["crate app (runtime) holds 2 file(s) of nav"])
        # The baseline holds the name, so more files of the same system in the same crate are not new.
        self.assertEqual(cairn.new_violations(more.violations, {"R1 app nav"}), [])

    def test_r4_only_the_tooling_area_lives_in_tools(self):
        docs = [doc("tooling", ["tools/ci.py"], area="Tooling"), doc("maps", ["tools/maps.py", "tools/tiles.py"])]
        result = analyse(docs, ["tools/ci.py", "tools/maps.py", "tools/tiles.py"])
        self.assertEqual(messages(result, "R4"), ["maps (Device) has 2 file(s) under tools/"])


class RatchetTests(unittest.TestCase):
    def failing(self, violations, baseline, approved):
        with redirect_stdout(io.StringIO()):
            docs = [doc("maps", []), doc("app", [], kind="shell")]
            return cairn.report(cairn.Result(violations), baseline, docs, approved)

    def test_a_new_violation_fails_only_for_an_approved_system(self):
        new = cairn.Violation("R4", "R4 maps", frozenset({"maps"}), "maps in tools")
        self.assertEqual(self.failing([new], set(), set()), [])
        self.assertEqual(self.failing([new], set(), {"maps"}), ["maps in tools"])
        self.assertEqual(self.failing([new], {"R4 maps"}, {"maps"}), [])

    def test_an_unowned_file_fails_once_any_system_is_approved(self):
        unowned = cairn.Violation("ownership", "unowned x.rs", frozenset(), "x.rs has no owner")
        self.assertEqual(self.failing([unowned], set(), set()), [])
        self.assertEqual(self.failing([unowned], set(), {"maps"}), ["x.rs has no owner"])

    def test_a_gone_entry_fails_only_for_an_approved_system(self):
        baseline = {"R4 maps", "R1 crate app"}
        self.assertEqual(self.failing([], baseline, set()), [])
        self.assertEqual(self.failing([], baseline, {"maps"}), ["R4 maps"])
        self.assertEqual(self.failing([], baseline, {"app"}), ["R1 crate app"])

    def test_update_never_grows_the_baseline(self):
        violations = [cairn.Violation("R4", "R4 maps", frozenset({"maps"}), ""),
                      cairn.Violation("R4", "R4 tiles", frozenset({"tiles"}), "")]
        self.assertEqual(cairn.updated(violations, {"R4 maps", "R1 gone"}), {"R4 maps"})

    def test_a_baseline_that_grew_by_hand_is_caught(self):
        current = [cairn.Violation("R4", "R4 maps", frozenset({"maps"}), "")]
        self.assertEqual(cairn.grown({"R4 maps"}, {"R4 maps"}, current), [])
        self.assertEqual(cairn.grown({"R4 maps", "R1 new"}, {"R4 maps"}, current), ["R1 new"])
        # With no baseline on the base, only the first baseline that `--accept` writes passes.
        self.assertEqual(cairn.grown({"R4 maps"}, None, current), [])
        self.assertEqual(cairn.grown({"R4 maps", "R1 new"}, None, current), ["R1 new"])

    def test_the_base_baseline_path_comes_from_the_base_config(self):
        files = {
            ".cairn/config.json": '{"baseline": "old/baseline.json"}',
            "old/baseline.json": '["R4 maps"]',
        }
        self.assertEqual(cairn.base_baseline(files.get), {"R4 maps"})
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
