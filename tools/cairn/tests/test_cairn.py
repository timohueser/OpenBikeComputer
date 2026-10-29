import importlib.util
import io
import json
import os
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


def header(name, owns, kind="system", status="draft"):
    area = "area: Device\n" if kind == "system" else ""
    return f"---\nsystem: {name}\nkind: {kind}\n{area}purpose: P.\nowns: [{owns}]\nstatus: {status}\n---\n# {name.title()}\n"


class Repository:
    """A git repository in a temporary folder; `commit` writes files, and None deletes one."""

    CONFIG = {
        "docs": "docs/design", "baseline": ".cairn/baseline.json", "specs": "specs",
        "fix": {"max_lines": 1000, "tests": ["**/tests/**"]},
        "sources": {"extensions": [".rs"], "files": []}, "adapters": {}, "runtimes": {},
        "tooling": {"path": "tools/", "area": "Tooling"},
    }

    def __init__(self, folder):
        self.root = Path(folder)
        self.git("init", "--quiet", "--initial-branch=topic")

    def git(self, *args):
        return subprocess.run(["git", "-C", str(self.root), "-c", "user.name=t", "-c", "user.email=t@t", *args],
                              check=True, capture_output=True, text=True).stdout.strip()

    def commit(self, files, **config):
        if config:
            files = {**files, ".cairn/config.json": json.dumps({**self.CONFIG, **config})}
        for path, text in files.items():
            target = self.root / path
            if text is None:
                target.unlink()
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(text)
        self.git("add", "-A")
        self.git("commit", "--quiet", "-m", "c")
        return self.git("rev-parse", "HEAD")

    def gate(self, base, line):
        """Run the mode gate on HEAD with a declared mode, and give (fails, report)."""
        cairn.modes.declare(self.root, cairn.modes.parse(line))
        config = json.loads((self.root / cairn.CONFIG).read_text())
        docs, files, units, edges, lines = cairn.gather(self.root, config)
        head = cairn.Tree(docs, config, cairn.analyse(docs, files, units, edges, lines, config).edges)
        out = io.StringIO()
        with mock.patch.dict(os.environ), redirect_stdout(out):
            os.environ.pop("GITHUB_EVENT_PATH", None)
            return cairn.mode_gate(self.root, head, base), out.getvalue()


class PullRequestDiffTests(unittest.TestCase):
    def test_the_diff_names_its_systems_doc_headers_specs_and_lines_outside_tests(self):
        config = {"docs": "docs/design", "specs": "specs", "fix": {"tests": ["**/tests/**"]}}
        # The website shell owns every doc, but a system doc touches only its own system.
        docs = [doc("routing", ["route/**"]), doc("search", ["search/**"]), doc("website", ["docs/**"], kind="shell")]
        with tempfile.TemporaryDirectory() as folder:
            repository = Repository(folder)
            base = repository.commit({"docs/design/routing.md": header("routing", "route/**"),
                                      "docs/design/search.md": header("search", "search/**")})
            repository.commit({
                "docs/design/routing.md": header("routing", "route/**", status="approved"),
                "docs/design/search.md": header("search", "search/**") + "Body.\n",
                "route/a.rs": "a\nb\n",
                "route/tests/a.rs": "t\n" * 50,
                "specs/route.md": "x\n",
            })
            diff = cairn.pr_diff(repository.root, base, docs, config, set())
        self.assertEqual(diff.touched, {"routing", "search"})
        self.assertEqual(diff.docs, {"routing", "search"})
        self.assertEqual(diff.headers, {"routing"})
        self.assertEqual(diff.specs, ("specs/route.md",))
        # The tests do not count: route/a.rs 2, the routing doc 2, the search doc 1, the spec 1.
        self.assertEqual(diff.lines, 6)

    def test_a_test_pattern_with_no_folder_matches_the_file_name_only(self):
        patterns = json.loads((cairn.ROOT / cairn.CONFIG).read_text())["fix"]["tests"]
        for path in ("tools/cairn/tests/test_mode.py", "a/src/tests.rs", "a/src/peaks_tests.rs", "web/A.svelte.test.ts"):
            self.assertTrue(cairn.is_test(path, patterns), path)
        for path in ("tools/test_plan.py", "a/tests_b/lib.rs", "tests.rs.orig"):
            self.assertFalse(cairn.is_test(path, patterns), path)

    def test_a_test_dependency_that_becomes_a_production_dependency_is_new(self):
        base = {("a", "b", True), ("a", "c", False)}
        self.assertEqual(cairn.added_edges({("a", "b", False), ("a", "c", False)}, base), {("a", "b")})
        self.assertEqual(cairn.added_edges({("a", "b", True), ("a", "c", True), ("c", "a", True)}, base), {("c", "a")})


class ModeGateTests(unittest.TestCase):
    def test_the_base_approval_and_fix_rules_hold_against_a_change_that_loosens_them(self):
        with tempfile.TemporaryDirectory() as folder:
            repository = Repository(folder)
            base = repository.commit({"docs/design/routing.md": header("routing", "route/**", status="approved"),
                                      "route/a.rs": "a\n"}, fix={"max_lines": 1, "tests": []})
            repository.commit({"docs/design/routing.md": header("routing", "route/**"), "route/a.rs": "b\nc\nd\n"},
                              fix={"max_lines": 1000, "tests": []})
            fails, report = repository.gate(base, "fix")
        self.assertTrue(fails, report)
        self.assertIn("a fix changes no doc header, and this one changes routing", report)
        self.assertIn("a fix changes at most 1 lines outside tests", report)

    def test_an_unreadable_base_is_a_violation_that_fails_only_for_an_approved_system(self):
        with tempfile.TemporaryDirectory() as folder:
            repository = Repository(folder)
            base = repository.commit({"route/a.rs": "a\n"})
            repository.commit({"docs/design/routing.md": header("routing", "route/**"), "route/a.rs": "b\n"}, fix={"max_lines": 1000, "tests": []})
            fails, report = repository.gate(base, "fix")
            self.assertFalse(fails, report)
            self.assertIn(f"the base {base[:12]} cannot be read", report)
            repository.commit({"docs/design/routing.md": header("routing", "route/**", status="approved")})
            fails, report = repository.gate(base, "fix")
            self.assertTrue(fails, report)

    def test_check_pr_fails_a_fix_over_the_limit_in_an_approved_system(self):
        docs = [doc("routing", ["route/**"], status="approved")]
        diff = cairn.modes.Diff(frozenset({"routing"}), lines=1001)
        with tempfile.TemporaryDirectory() as folder:
            event = Path(folder) / "event.json"
            event.write_text(json.dumps({"pull_request": {"body": "Mode: fix"}}))
            with mock.patch.dict(os.environ, {"GITHUB_EVENT_PATH": str(event)}), \
                    mock.patch.object(cairn, "gather", return_value=(docs, [], [], [], {})), \
                    mock.patch.object(cairn, "merge_base", return_value="0" * 40), \
                    mock.patch.object(cairn, "pr_diff", return_value=diff), \
                    redirect_stdout(io.StringIO()) as out:
                code = cairn.main(["check", "--pr", "--base", "develop"])
        self.assertEqual(code, 1, out.getvalue())
        self.assertIn("a fix changes at most 1,000 lines outside tests", out.getvalue())


if __name__ == "__main__":
    unittest.main()
