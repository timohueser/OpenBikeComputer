from __future__ import annotations

import copy
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import suite_registry as registry

class SuiteRegistryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        (self.root / "src").mkdir()
        (self.root / "src/lib.rs").write_text("pub fn value() -> u8 { 1 }\n", encoding="utf-8")
        (self.root / "Cargo.toml").write_text("[package]\nname='demo'\nversion='0.1.0'\n", encoding="utf-8")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def metadata(self, _root: Path, manifest: Path | None) -> dict:
        name = "demo" if manifest is None else manifest.parent.name
        manifest_path = self.root / "Cargo.toml" if manifest is None else manifest
        return {
            "packages": [
                {
                    "name": name,
                    "manifest_path": str(manifest_path),
                    "targets": [{"name": name, "kind": ["lib"], "test": True}],
                }
            ]
        }

    def base_documents(self) -> tuple[dict, dict, list[registry.Discovered]]:
        suites = {
            "schema": 1,
            "suite": [
                {
                    "id": "demo",
                    "surface": "rust",
                    "level": "unit",
                    "command": "cargo test -p demo --lib --locked",
                    "fixtures": [],
                    "pull_request": "affected",
                    "scheduled": "none",
                    "extra_triggers": [],
                    "ownership": [{"kind": "rust-package", "name": "demo"}],
                    "coverage_component": "app",
                }
            ],
        }
        coverage = {
            "schema": 1,
            "component": [
                {"id": "app", "include": ["src/**"], "exclude": [], "enforcement": "report"}
            ],
        }
        discovered = [registry.Discovered("rust-target", "demo:demo", "Cargo.toml", "lib")]
        return suites, coverage, discovered

    def test_discovers_every_required_source(self) -> None:
        files = {
            "builder/app/src/example.test.ts": "test('x', () => {})",
            "apps/obc-web-demo/tests/browser/demo.test.js": "test('x', () => {})",
            "tools/tests/test_tool.py": "def test_x(): pass",
            "firmware/tools/tests/test_firmware.py": "def test_x(): pass",
            "builder/tests/test_builder.py": "def test_x(): pass",
            "companion-ios/OBCCompanionUITests/SmokeTests.swift": "func testSmoke() {}",
            "companion-ios/Packages/OBCKit/Package.swift": '.testTarget(name: "OBCKitTests")',
            ".github/workflows/ci.yml": "jobs:\n  test:\n    steps:\n      - run: cargo test --workspace --locked\n",
        }
        for relative, content in files.items():
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
        discovered = registry.discover_all(self.root, self.metadata)
        kinds = {item.kind for item in discovered}
        self.assertEqual(
            kinds,
            {
                "rust-target",
                "web-test",
                "browser-test",
                "python-test",
                "xcuitest",
                "swift-target",
                "workflow-command",
            },
        )

    def test_command_package_flags_belong_to_cargo(self) -> None:
        suites, coverage, discovered = self.base_documents()
        suites["suite"][0]["command"] = "mkdir -p reports && cargo test -p demo --locked"
        registry.validate(self.root, suites, coverage, discovered)
        suites["suite"][0]["command"] = "mkdir -p reports && cargo test -p missing --locked"
        with self.assertRaisesRegex(registry.RegistryError, "unknown Cargo package missing"):
            registry.validate(self.root, suites, coverage, discovered)

    def test_step_condition_does_not_replace_job_selection_gate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workflow = root / '.github/workflows/ci.yml'
            workflow.parent.mkdir(parents=True)
            workflow.write_text("jobs:\n  test:\n    runs-on: ubuntu-latest\n    needs: selection\n"
                                "    if: contains(fromJSON(needs.selection.outputs.jobs), 'test')\n"
                                "    steps:\n      - name: optional suite\n"
                                "        if: contains(fromJSON(needs.selection.outputs.plan).selected_suite_ids, 'ci.ui-snapshots')\n"
                                "        run: echo run\n")
            self.assertEqual(registry.workflow_jobs(root)['test'].gates_on, 'test')


    def test_list_data_is_derived_not_stored(self) -> None:
        suites, coverage, discovered = self.base_documents()
        inventory = registry.validate(self.root, suites, coverage, discovered)
        row = registry._suite_summary(inventory.suites[0], inventory.matches["demo"])
        self.assertEqual(row["discovered_units"], 1)

    def test_rejection_rules(self) -> None:
        cases = []

        def case(name, mutate, expected):
            cases.append((name, mutate, expected))

        case("duplicate id", lambda s, c, d: s["suite"].append(copy.deepcopy(s["suite"][0])), "duplicate suite IDs")
        case("orphan", lambda s, c, d: s["suite"][0].update(ownership=[{"kind": "rust-package", "name": "other"}]), "orphan discovered suite")
        case("invalid level", lambda s, c, d: s["suite"][0].update(level="integration"), "invalid level")
        case("invalid PR cadence", lambda s, c, d: s["suite"][0].update(pull_request="sometimes"), "invalid pull_request cadence")
        case("invalid schedule", lambda s, c, d: s["suite"][0].update(scheduled="monthly"), "invalid scheduled cadence")
        case("fixture declaration", lambda s, c, d: s["suite"][0].update(level="fixture"), "must declare fixtures")
        case("derived fact", lambda s, c, d: s["suite"][0].update(test_count=3), "derived fields are forbidden")
        case("budget issue", lambda s, c, d: s["suite"][0].update(budget_exception={"reason": "slow"}), "GitHub issue reference")
        case("quarantine issue", lambda s, c, d: s["suite"][0].update(quarantine={"reason": "flake"}), "GitHub issue reference")
        case("global coverage exclusion", lambda s, c, d: c.update(exclude=[{"path": "src/**"}]), "global exclusion needs path and replacement evidence")
        case("unknown component", lambda s, c, d: s["suite"][0].update(coverage_component="missing"), "unknown coverage component")
        case("dead entry", lambda s, c, d: s["suite"].append({**copy.deepcopy(s["suite"][0]), "id": "dead", "ownership": [{"kind": "workflow", "pattern": "never"}]}), "dead registry entry")
        case("missing trigger", lambda s, c, d: s["suite"][0].update(extra_triggers=["missing/**"]), "extra trigger matches no maintained path")
        case("live PR", lambda s, c, d: s["suite"][0].update(level="live", pull_request="always"), "cannot be required")
        case("live command", lambda s, c, d: s["suite"][0].update(command="curl https://example.com"), "appears to contact a live service")
        case("missing command path", lambda s, c, d: s["suite"][0].update(command="tools/missing.py check"), "command path does not exist")
        case("missing working directory", lambda s, c, d: s["suite"][0].update(command="cd missing && python3 -m unittest"), "working directory does not exist")
        case("bad safety policy", lambda s, c, d: c["component"].append({"id": "crc", "include": ["src/**"], "enforcement": "report"}), "must be planned as ratchet")
        case("invented baseline", lambda s, c, d: c["component"][0].update(baseline="pending"), "omit pending baseline")
        case("exclusion evidence", lambda s, c, d: c["component"][0].update(exclude=[{"path": "src/generated.rs"}]), "replacement evidence")

        for name, mutate, expected in cases:
            with self.subTest(name=name):
                suites, coverage, discovered = self.base_documents()
                mutate(suites, coverage, discovered)
                with self.assertRaisesRegex(registry.RegistryError, expected):
                    registry.validate(self.root, suites, coverage, discovered)

    def test_duplicate_ownership_is_rejected(self) -> None:
        suites, coverage, discovered = self.base_documents()
        duplicate = copy.deepcopy(suites["suite"][0])
        duplicate["id"] = "demo-too"
        suites["suite"].append(duplicate)
        with self.assertRaisesRegex(registry.RegistryError, "duplicate ownership"):
            registry.validate(self.root, suites, coverage, discovered)

    def test_python_sleep_detection_uses_call_syntax(self) -> None:
        source = self.root / "tools/tests/test_wait.py"
        source.parent.mkdir(parents=True)
        suites, coverage, _ = self.base_documents()
        suites["suite"][0].update(
            surface="python",
            command="python3 -m unittest tools.tests.test_wait",
            ownership=[{"kind": "path", "source": "python-test", "pattern": "tools/tests/test_*.py"}],
        )
        discovered = [registry.Discovered("python-test", "tools/tests/test_wait.py", "tools/tests/test_wait.py")]
        for text in ['# time.sleep(1)\n', 'fixture = "time.sleep(1)"\n', 'fixture = "Task.sleep(1)"\n']:
            with self.subTest(source=text):
                source.write_text(text, encoding="utf-8")
                registry.validate(self.root, suites, coverage, discovered)
        for content in [b"def broken(:\n", b"\xff"]:
            with self.subTest(source=content):
                source.write_bytes(content)
                with self.assertRaisesRegex(registry.RegistryError, "cannot inspect tools/tests/test_wait.py"):
                    registry.validate(self.root, suites, coverage, discovered)
        source.write_text("import time\ntime.sleep(1)\n", encoding="utf-8")
        with self.assertRaisesRegex(registry.RegistryError, "real sleep"):
            registry.validate(self.root, suites, coverage, discovered)
        suites["suite"][0]["sleep_exception"] = {"reason": "bounded watchdog", "issue": "#1449"}
        registry.validate(self.root, suites, coverage, discovered)

    def test_workflow_parser_handles_inline_blocks_and_matrix_commands(self) -> None:
        workflow = self.root / ".github/workflows/ci.yml"
        workflow.parent.mkdir(parents=True)
        workflow.write_text(
            "on:\n"
            "  pull_request:\n"
            "jobs:\n"
            "  test:\n"
            "    steps:\n"
            "      - run: npm test\n"
            "      - run: |\n"
            "          python3 tools/check.py\n"
            "          echo ignored\n"
            '      - { name: test, cmd: "cargo test --locked" }\n'
            "  board:\n"
            "    defaults:\n"
            "      run:\n"
            "        working-directory: crates/board\n"
            "    steps:\n"
            "      - run: cargo build --release\n",
            encoding="utf-8",
        )
        steps = {(item.command, item.job, item.working_directory) for item in registry.scan_workflow(self.root)}
        self.assertEqual(
            steps,
            {
                ("npm test", "test", ""),
                ("python3 tools/check.py", "test", ""),
                ("cargo test --locked", "test", ""),
                ("cargo build --release", "board", "crates/board"),
            },
        )
        # The `on:` block is not a job, so its keys never become CI routes.
        self.assertEqual(set(registry.workflow_jobs(self.root)), {"test", "board"})

    def test_a_bare_assignment_is_shell_state_but_an_env_prefix_is_a_command(self) -> None:
        """`VAR=value` alone sets shell state; `VAR=value cmd` still runs, and still routes.

        Splitting `export VAR="$(cmd)"` into an assignment and an `export` is what shellcheck's
        SC2155 asks for, so the command's exit status stops being swallowed. That split must not
        turn the assignment half into a discovered execution unit needing a registry owner. The
        line is the whole distinction: an assignment *is* the line, an env prefix has a command
        after it.
        """

        workflow = self.root / ".github/workflows/ci.yml"
        workflow.parent.mkdir(parents=True)
        workflow.write_text(
            "on:\n"
            "  pull_request:\n"
            "jobs:\n"
            "  test:\n"
            "    steps:\n"
            "      - run: |\n"
            '          OBC_FIXTURE_ROOT="$(python3 tools/fixtures.py root)"\n'
            "          export OBC_FIXTURE_ROOT\n"
            "          PLAIN=python3\n"
            "          QUOTED='python3 tools/decoy.py'\n"
            "          EMPTY=\n"
            "          PYTHONPATH=. python3 -m pytest builder/tests/ -v\n"
            "          OBC_LOG=trace cargo test -p demo --locked\n",
            encoding="utf-8",
        )
        self.assertEqual(
            {item.command for item in registry.scan_workflow(self.root)},
            {
                "PYTHONPATH=. python3 -m pytest builder/tests/ -v",
                "OBC_LOG=trace cargo test -p demo --locked",
            },
        )
        # …and the env-prefixed cargo line is a real invocation, so it still routes its package.
        graph = registry.CargoGraph(
            packages={
                "demo": registry.CargoPackage("demo", "crates/demo", "crates/demo/Cargo.toml", frozenset())
            },
            reverse_dependencies={},
        )
        self.assertEqual(registry.cargo_job_coverage(self.root, graph), {"demo": {"test"}})

class SuiteSelectionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.suites = [
            self._suite("rust.core", "rust-package", "core", triggers=["specs/vectors/**"]),
            self._suite("rust.leaf", "rust-package", "leaf"),
            self._suite("rust.consumer", "rust-package", "consumer", level="contract", triggers=["specs/vectors/**"]),
            self._suite("rust.desktop", "rust-package", "desktop", platforms=["linux", "macos", "windows"]),
            self._suite("web.unit", "path", "web-test", pattern="web/**/*.test.ts", triggers=["web/src/**", "specs/vectors/**"]),
            self._suite("swift.contract", "swift-target", "SwiftTests", package="ios/Package.swift", triggers=["specs/vectors/**"]),
            self._suite("python.service", "path", "python-test", pattern="service/tests/test_*.py", triggers=["service/**"]),
            self._suite("python.tool", "path", "python-test", pattern="tools/tests/test_*.py", triggers=["tools/tool.py"]),
            self._suite("python.builder", "path", "python-test", pattern="builder/tests/test_*.py", triggers=["builder/server/**/*.py"]),
            self._suite("fixture.consumer", "path", "python-test", pattern="fixtures/tests/test_*.py", fixtures=["catalog"]),
            self._suite("missing.route", "path", "browser-test", pattern="demo/tests/*.test.ts", triggers=["demo/**"]),
            self._suite("ci.policy", "workflow", "python3 tools/check.py", triggers=["testing/**", ".github/workflows/**"]),
            self._suite(
                "ci.dependencies",
                "workflow",
                "python3 firmware/tools/check_dependencies.py",
                triggers=["firmware/tools/dependency_rules.json"],
            ),
            self._suite("ci.docs", "workflow", "python3 docs/build.py", triggers=["docs/**"]),
        ]
        matches = {suite["id"]: [] for suite in self.suites}
        self.inventory = registry.Inventory(self.suites, [], [], matches)
        packages = {
            "core": registry.CargoPackage("core", "crates/core", "crates/core/Cargo.toml", frozenset()),
            "leaf": registry.CargoPackage("leaf", "crates/leaf", "crates/leaf/Cargo.toml", frozenset({"core"})),
            "consumer": registry.CargoPackage("consumer", "crates/consumer", "crates/consumer/Cargo.toml", frozenset({"leaf"})),
            "desktop": registry.CargoPackage("desktop", "apps/desktop", "apps/desktop/Cargo.toml", frozenset(), False),
        }
        self.graph = registry.CargoGraph(
            packages,
            {
                "core": frozenset({"leaf"}),
                "leaf": frozenset({"consumer"}),
                "consumer": frozenset(),
                "desktop": frozenset(),
            },
        )
        self.routes = {suite["id"]: ["test"] for suite in self.suites}
        self.routes.update(
            {
                "rust.desktop": ["desktop"],
                "web.unit": ["web"],
                "swift.contract": ["ios-unit"],
                "python.tool": ["policy"],
                "fixture.consumer": ["policy"],
                "ci.policy": ["policy"],
                "ci.docs": ["docs"],
                "missing.route": [],
            }
        )
        self.unconditional = {"policy"}

    def tearDown(self) -> None:
        self.temporary.cleanup()

    @staticmethod
    def _suite(
        suite_id,
        owner_kind,
        owner_name,
        *,
        pattern=None,
        package=None,
        triggers=None,
        fixtures=None,
        level="unit",
        platforms=None,
    ):
        owner = {"kind": owner_kind}
        if owner_kind == "rust-package":
            owner["name"] = owner_name
        elif owner_kind == "path":
            owner.update(source=owner_name, pattern=pattern)
        elif owner_kind == "swift-target":
            owner.update(name=owner_name, package=package)
        else:
            owner["pattern"] = owner_name
        suite = {
            "id": suite_id,
            "surface": suite_id.split(".", 1)[0],
            "level": level,
            "command": f"run {suite_id}",
            "fixtures": fixtures or [],
            "pull_request": "affected",
            "scheduled": "none",
            "extra_triggers": triggers or [],
            "ownership": [owner],
        }
        if platforms:
            suite["platforms"] = platforms
        return suite

    def plan(self, path):
        return registry.select_suites(
            self.inventory, [path], self.graph, self.routes, unconditional=self.unconditional
        )

    def selected(self, path):
        return {selection.suite["id"] for selection in self.plan(path).selected}

    def test_required_path_and_graph_cases(self) -> None:
        cases = {
            "Cargo.toml": {"rust.core", "rust.leaf", "rust.consumer"},
            "Cargo.lock": {"rust.core", "rust.leaf", "rust.consumer"},
            "rust-toolchain.toml": {"rust.core", "rust.leaf", "rust.consumer", "rust.desktop"},
            "rustfmt.toml": {"rust.core", "rust.leaf", "rust.consumer", "rust.desktop"},
            ".cargo/config.toml": {"rust.core", "rust.leaf", "rust.consumer", "rust.desktop"},
            "crates/leaf/src/lib.rs": {"rust.leaf", "rust.consumer"},
            "crates/core/src/lib.rs": {"rust.core", "rust.leaf", "rust.consumer"},
            "crates/core/tests/common/mod.rs": {"rust.core", "rust.leaf", "rust.consumer"},
            "specs/vectors/format.json": {"rust.core", "rust.consumer", "web.unit", "swift.contract"},
            "service/probe.py": {"python.service"},
            "tools/tool.py": {"python.tool"},
            "builder/server/nested/routes.py": {"python.builder"},
            "fixtures/catalog.toml": {"fixture.consumer"},
            ".github/workflows/release.yml": {"ci.policy"},
            "firmware/tools/dependency_rules.json": {"ci.dependencies"},
            "testing/suites.toml": {"ci.policy"},
            "web/src/view.ts": {"web.unit"},
            "ios/Sources/Model.swift": {"swift.contract"},
            "apps/desktop/src/main.rs": {"rust.desktop"},
            "docs/guide.md": {"ci.docs"},
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                self.assertTrue(expected.issubset(self.selected(path)))

    def test_reverse_dependency_reasons_name_the_cargo_edge(self) -> None:
        plan = self.plan("crates/core/src/lib.rs")
        reasons = next(item.reasons for item in plan.suites if item.suite["id"] == "rust.consumer")
        self.assertTrue(any("reverse dependency consumer compiles leaf" in reason for reason in reasons))

    def test_cargo_graph_is_derived_from_synthetic_metadata(self) -> None:
        def metadata(_root, _manifest):
            core_id = "path+file:///repo/core#0.1.0"
            leaf_id = "path+file:///repo/leaf#0.1.0"
            return {
                "workspace_members": [core_id, leaf_id],
                "packages": [
                    {
                        "id": core_id,
                        "name": "core",
                        "manifest_path": str(self.root / "crates/core/Cargo.toml"),
                        "dependencies": [],
                    },
                    {
                        "id": leaf_id,
                        "name": "leaf",
                        "manifest_path": str(self.root / "crates/leaf/Cargo.toml"),
                        "dependencies": [{"name": "core"}],
                    },
                ],
            }

        graph = registry.build_cargo_graph(self.root, metadata)
        self.assertEqual(graph.packages["leaf"].dependencies, frozenset({"core"}))
        self.assertEqual(graph.reverse_dependencies["core"], frozenset({"leaf"}))
        self.assertTrue(graph.packages["core"].root_workspace)

    def test_unknown_production_path_fails_closed(self) -> None:
        plan = self.plan("unknown/new_source.py")
        self.assertRegex(plan.errors[0], "no suite owner")

    def test_selected_suite_without_route_fails_closed(self) -> None:
        plan = self.plan("demo/src/index.ts")
        self.assertIn("missing.route", {item.suite["id"] for item in plan.selected})
        self.assertTrue(any("no executable CI route" in error for error in plan.errors))

    def test_manual_fixture_stays_owned_but_is_not_selected_for_ci(self) -> None:
        suite = self._suite(
            "fixture.manual", "path", "fixture-validation",
            pattern="fixtures/verify-inputs.py", fixtures=["captured-inputs"],
            triggers=["fixtures/sources/**"], level="fixture",
        )
        suite["pull_request"] = "never"
        suite["scheduled"] = "manual"
        self.suites.append(suite)
        for path in ("fixtures/verify-inputs.py", "fixtures/sources/input.json"):
            with self.subTest(path=path):
                plan = self.plan(path)
                self.assertFalse(plan.errors)
                self.assertNotIn("fixture.manual", {item.suite["id"] for item in plan.selected})

    def test_json_reports_platforms_jobs_and_non_selected_reasons(self) -> None:
        jobs = {"desktop": registry.WorkflowJob("desktop", "ubuntu-latest", ("bundle",), True)}
        data = registry.selection_plan_data(self.plan("apps/desktop/src/main.rs"), jobs)
        desktop = next(item for item in data["suites"] if item["id"] == "rust.desktop")
        web = next(item for item in data["suites"] if item["id"] == "web.unit")
        self.assertEqual(desktop["platforms"], ["linux", "macos", "windows"])
        self.assertEqual(desktop["jobs"], ["desktop"])
        self.assertEqual(web["reasons"], [registry.NOT_SELECTED])
        # The plan's job list closes over the workflow `needs` graph so producers still run.
        self.assertIn("bundle", data["required_jobs"])

    def test_git_diff_input_works_in_a_temporary_repository(self) -> None:
        subprocess.run(["git", "init", "-q"], cwd=self.root, check=True)
        subprocess.run(["git", "config", "user.email", "tests@example.com"], cwd=self.root, check=True)
        subprocess.run(["git", "config", "user.name", "Tests"], cwd=self.root, check=True)
        source = self.root / "web/src/view.ts"
        source.parent.mkdir(parents=True)
        source.write_text("export const value = 1;\n", encoding="utf-8")
        subprocess.run(["git", "add", "."], cwd=self.root, check=True)
        subprocess.run(["git", "commit", "-qm", "base"], cwd=self.root, check=True)
        base = subprocess.run(["git", "rev-parse", "HEAD"], cwd=self.root, check=True, capture_output=True, text=True).stdout.strip()
        source.write_text("export const value = 2;\n", encoding="utf-8")
        subprocess.run(["git", "commit", "-qam", "head"], cwd=self.root, check=True)
        self.assertEqual(registry.git_changed_paths(self.root, base, "HEAD"), ["web/src/view.ts"])

class LocalInterfaceTests(unittest.TestCase):
    """The level, surface, and dry-run behavior of `obc test`, on a synthetic registry."""

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.marker = self.root / "ran"
        self.suites = [
            self._suite("unit.formats", "unit", "formats"),
            self._suite("unit.maps", "unit", "maps"),
            self._suite("component.maps", "component", "maps"),
            self._suite("contract.formats", "contract", "formats"),
            self._suite("fixture.map", "fixture", "map"),
            self._suite("e2e.ios", "end-to-end", "ios", platforms=["nonexistent-os"]),
        ]
        self.inventory = registry.Inventory(self.suites, [], [], {s["id"]: [] for s in self.suites})
        self.routes = {suite["id"]: ["test"] for suite in self.suites}

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def _suite(self, suite_id, level, surface, platforms=None):
        suite = {
            "id": suite_id,
            "surface": surface,
            "level": level,
            "command": f"echo {suite_id} >> ran",
            "fixtures": [],
            "pull_request": "affected",
            "scheduled": "none",
            "extra_triggers": [],
            "ownership": [],
        }
        if platforms:
            suite["platforms"] = platforms
        return suite

    def selected(self, level, surface=None):
        plan = registry.select_by_level(self.inventory, self.routes, level, surface)
        return [selection.suite["id"] for selection in plan.selected]

    def test_level_and_surface_selection(self) -> None:
        cases = [
            (("unit", None), ["unit.formats", "unit.maps"]),
            (("unit", "maps"), ["unit.maps"]),
            (("component", None), ["component.maps"]),
            (("contract", None), ["contract.formats"]),
            (("fixtures", None), ["fixture.map"]),
            (("fixture", None), ["fixture.map"]),
            (("e2e", None), ["e2e.ios"]),
            (("end-to-end", None), ["e2e.ios"]),
        ]
        for (level, surface), expected in cases:
            with self.subTest(level=level, surface=surface):
                self.assertEqual(self.selected(level, surface), expected)

    def test_unknown_level_surface_and_alias_are_rejected(self) -> None:
        for level, surface, expected in [
            ("integration", None, "unknown test level"),
            ("fixture-tests", None, "unknown test level"),
            ("end2end", None, "unknown test level"),
            ("unit", "storage", "unknown surface"),
        ]:
            with self.subTest(level=level, surface=surface):
                with self.assertRaisesRegex(registry.RegistryError, expected):
                    registry.select_by_level(self.inventory, self.routes, level, surface)

    def test_missing_ci_route_is_an_error_in_the_run_form(self) -> None:
        routes = dict(self.routes, **{"unit.maps": []})
        plan = registry.select_by_level(self.inventory, routes, "unit")
        self.assertTrue(any("no executable CI route" in error for error in plan.errors))
        self.assertEqual(registry.run_plan(plan, self.root), 1)
        self.assertFalse(self.marker.exists())

    def test_dry_run_executes_nothing(self) -> None:
        plan = registry.select_by_level(self.inventory, self.routes, "unit")
        self.assertEqual(registry.run_plan(plan, self.root, dry_run=True), 0)
        self.assertFalse(self.marker.exists())

    def test_manual_fixture_runs_explicitly_without_a_ci_route(self) -> None:
        suite = next(suite for suite in self.suites if suite["id"] == "fixture.map")
        suite["pull_request"] = "never"
        suite["scheduled"] = "manual"
        self.routes[suite["id"]] = []
        plan = registry.select_by_level(self.inventory, self.routes, "fixtures", "map")
        self.assertFalse(plan.errors)
        self.assertFalse(plan.selected)
        arguments = registry.build_parser().parse_args(["--root", str(self.root), "run", "--scheduled", "manual", "--surface", "map"])
        with patch.object(registry, "load_inventory", return_value=self.inventory), \
             patch.object(registry, "build_cargo_graph", return_value=registry.CargoGraph({}, {})), \
             patch.object(registry, "suite_workflow_jobs", return_value=self.routes):
            self.assertEqual(registry.command_run(arguments), 0)
        self.assertEqual(self.marker.read_text(), "fixture.map\n")

    def test_platform_restricted_suite_is_skipped_not_passed(self) -> None:
        plan = registry.select_by_level(self.inventory, self.routes, "e2e")
        self.assertEqual(registry.run_plan(plan, self.root), 0)
        self.assertFalse(self.marker.exists())

    def test_run_stops_on_the_first_failing_suite(self) -> None:
        self.suites[0]["command"] = "exit 3"
        plan = registry.select_by_level(self.inventory, self.routes, "unit")
        self.assertEqual(registry.run_plan(plan, self.root), 1)
        self.assertFalse(self.marker.exists())

    def test_affected_without_a_base_is_an_actionable_failure(self) -> None:
        arguments = registry.build_parser().parse_args(["run", "--affected"])
        with self.assertRaisesRegex(registry.RegistryError, "--base origin/develop"):
            registry.command_run(arguments)

def synthetic_workflow(runs_on: str = "    runs-on: ubuntu-latest\n") -> str:
    return (
        "name: CI\non:\n  pull_request:\njobs:\n"
        "  selection:\n    runs-on: ubuntu-latest\n    steps:\n"
        "      - run: python3 tools/suite_registry.py select --base x\n"
        "  unit:\n    needs: selection\n"
        "    if: contains(fromJSON(needs.selection.outputs.jobs), 'unit')\n"
        f"{runs_on}    steps:\n      - run: cargo test --workspace --locked\n"
        "  ci:\n    needs: [selection, unit]\n    runs-on: ubuntu-latest\n    steps:\n"
        "      - run: python3 tools/ci_aggregate.py\n"
    )

def synthetic_suite(suite_id, command, ownership, pull_request="affected"):
    return {
        "id": suite_id,
        "surface": suite_id.split(".", 1)[0],
        "level": "unit",
        "command": command,
        "fixtures": [],
        "pull_request": pull_request,
        "scheduled": "none",
        "extra_triggers": [],
        "ownership": ownership,
    }

class CiRoutingTests(unittest.TestCase):
    """Job routing and its four post-cutover validation failures, on a synthetic workflow."""

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.workflow = self.root / ".github/workflows/ci.yml"
        self.workflow.parent.mkdir(parents=True)
        self.workflow.write_text(synthetic_workflow(), encoding="utf-8")
        self.policy_command = "python3 tools/suite_registry.py select --base x"
        self.suites = [
            synthetic_suite("rust.core", "cargo test -p core --locked", [{"kind": "rust-package", "name": "core"}]),
            synthetic_suite(
                "ci.policy",
                "obc check fmt",
                [{"kind": "workflow", "pattern": "python3 tools/suite_registry.py *"}],
                "always",
            ),
        ]
        matches = {
            "rust.core": [],
            "ci.policy": [
                registry.Discovered(
                    "workflow-command", self.policy_command, ".github/workflows/ci.yml", "selection"
                )
            ],
        }
        self.inventory = registry.Inventory(self.suites, [], [], matches)
        self.graph = registry.CargoGraph(
            {"core": registry.CargoPackage("core", "crates/core", "crates/core/Cargo.toml", frozenset())},
            {"core": frozenset()},
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def routes(self):
        return registry.suite_workflow_jobs(self.inventory, self.root, self.graph)

    def test_routing_is_derived_from_workflow_commands_and_cargo_coverage(self) -> None:
        # The aggregate gate reports suites; it is never a route for one.
        self.assertEqual(registry.aggregate_job(self.root), "ci")
        self.assertEqual(self.routes(), {"rust.core": ["unit"], "ci.policy": ["selection"]})
        self.assertEqual(registry.unconditional_jobs(self.root), {"selection"})

    def test_validation_rejects_the_post_cutover_failures(self) -> None:
        cases = [
            ("unrouted suite", lambda routes: routes.update(**{"rust.core": ["ghost"]}), "unknown workflow job ghost"),
            ("unrouted job", lambda routes: routes.update(**{"rust.core": []}), "job unit runs no registry suite"),
        ]
        for name, mutate, expected in cases:
            with self.subTest(name=name):
                routes = self.routes()
                mutate(routes)
                with self.assertRaisesRegex(registry.RegistryError, expected):
                    registry.validate_ci_routing(self.root, self.inventory, self.graph, routes)

    def test_a_trunk_build_routes_the_package_its_html_target_links(self) -> None:
        self.workflow.write_text(
            synthetic_workflow().replace(
                "      - run: cargo test --workspace --locked\n",
                "      - run: trunk build --config site/Trunk.toml\n",
            ),
            encoding="utf-8",
        )
        (self.root / "site").mkdir()
        (self.root / "site/Trunk.toml").write_text('[build]\ntarget = "page.html"\n', encoding="utf-8")
        (self.root / "site/page.html").write_text(
            '<link data-trunk rel="rust" href="../crates/core/Cargo.toml" data-wasm-opt="z" />\n',
            encoding="utf-8",
        )
        self.assertEqual(self.routes()["rust.core"], ["unit"])

    def test_validation_rejects_a_gate_that_names_another_job(self) -> None:
        self.workflow.write_text(
            synthetic_workflow().replace("jobs), 'unit')", "jobs), 'unitx')"), encoding="utf-8"
        )
        with self.assertRaisesRegex(registry.RegistryError, "gates on unitx"):
            registry.validate_ci_routing(self.root, self.inventory, self.graph, self.routes())

    def test_validation_rejects_a_job_outside_the_aggregate_gate(self) -> None:
        self.workflow.write_text(
            synthetic_workflow().replace("needs: [selection, unit]", "needs: [selection]"),
            encoding="utf-8",
        )
        with self.assertRaisesRegex(registry.RegistryError, "not in the aggregate gate's needs"):
            registry.validate_ci_routing(self.root, self.inventory, self.graph, self.routes())

    def test_validation_rejects_an_ungated_job_hosting_an_affected_suite(self) -> None:
        self.workflow.write_text(
            synthetic_workflow().replace(
                "    if: contains(fromJSON(needs.selection.outputs.jobs), 'unit')\n", ""
            ),
            encoding="utf-8",
        )
        with self.assertRaisesRegex(registry.RegistryError, "runs unconditionally but hosts"):
            registry.validate_ci_routing(self.root, self.inventory, self.graph, self.routes())

    def test_validation_rejects_an_unprovisioned_runner_image(self) -> None:
        self.workflow.write_text(synthetic_workflow(runs_on=""), encoding="utf-8")
        with self.assertRaisesRegex(registry.RegistryError, "provisions no runner image"):
            registry.validate_ci_routing(self.root, self.inventory, self.graph, self.routes())

    def test_validation_rejects_a_leftover_suite_policy_filter(self) -> None:
        self.workflow.write_text(
            synthetic_workflow().replace(
                f"      - run: {self.policy_command}\n",
                f"      - run: {self.policy_command}\n"
                "      - uses: dorny/paths-filter@v3\n        with:\n          filters: |\n"
                "            rust:\n              - 'testing/**'\n",
            ),
            encoding="utf-8",
        )
        with self.assertRaisesRegex(registry.RegistryError, "encodes suite policy"):
            registry.validate_ci_routing(self.root, self.inventory, self.graph, self.routes())

    def test_gate_claims_come_from_obc_check_commands(self) -> None:
        self.assertEqual(registry.gate_claims(self.suites), {"fmt": {"ci.policy"}})
        (self.root / "testing").mkdir()
        (self.root / "testing/suites.toml").write_text(
            'schema = 1\n[[suite]]\nid = "ci.policy"\ncommand = "obc check fmt"\n', encoding="utf-8"
        )
        arguments = registry.build_parser().parse_args(["--root", str(self.root), "gates", "clippy"])
        with self.assertRaisesRegex(registry.RegistryError, "reproduce no registry suite"):
            registry.command_gates(arguments)

    def test_a_workspace_gate_claims_every_package_suite_it_compiles(self) -> None:
        suites = [
            dict(
                self.suites[1],
                command="obc check test",
                ownership=[{"kind": "workflow", "pattern": "cargo test --workspace --locked"}],
            ),
            self.suites[0],
        ]
        graph = registry.CargoGraph(
            {"core": registry.CargoPackage("core", "crates/core", "crates/core/Cargo.toml", frozenset(), True)},
            {"core": frozenset()},
        )
        self.assertEqual(registry.gate_claims(suites, graph)["test"], {"ci.policy", "rust.core"})

    def test_fast_workspace_gate_does_not_claim_fixture_or_manual_execution(self):
        fast = {**self.suites[0], "level": "unit", "pull_request": "affected"}
        fixtures = {**fast, "id": "fixtures.core", "level": "fixture"}
        manual = {**fast, "id": "manual.core", "pull_request": "never"}
        gate = {**self.suites[1], "command": "obc check test", "ownership": [{"kind": "workflow", "pattern":
            'cargo nextest run --workspace --filter-expr "$(python3 tools/suite_registry.py cargo-filter --tier fast)"'}]}
        graph = registry.CargoGraph({"core": registry.CargoPackage("core", "crates/core", "crates/core/Cargo.toml", frozenset(), True)},
                                    {"core": frozenset()})
        self.assertEqual(registry.gate_claims([fast, fixtures, manual, gate], graph)["test"], {"ci.policy", "rust.core"})

class ShippedRoutingTests(unittest.TestCase):
    """The shipped registry's own routing, so a real change class cannot drift silently."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.root = registry.repository_root()
        cls.inventory = registry.load_inventory(cls.root)
        cls.graph = registry.build_cargo_graph(cls.root)
        cls.routes = registry.suite_workflow_jobs(cls.inventory, cls.root, cls.graph)
        cls.jobs = registry.workflow_jobs(cls.root)
        cls.unconditional = registry.unconditional_jobs(cls.root)

    def jobs_for(self, *paths):
        plan = registry.select_suites(
            self.inventory, list(paths), self.graph, self.routes, unconditional=self.unconditional
        )
        self.assertEqual(plan.errors, [])
        return set(registry.required_jobs(plan, self.jobs)) | self.unconditional

    def test_every_suite_routes_to_the_job_that_executes_it(self) -> None:
        expected = {
            "e2e.desktop-linux-launch": ["desktop-launch"],
            "rust.obc-crc": ["clippy", "fmt", "test"],
            "rust.obc-fw-nrf54l": ["embedded", "fmt"],
            "rust.obc-boot": ["boot", "fmt"],
            "rust.obc-desktop": ["desktop", "fmt"],
            "rust.obc-web-convert": ["clippy", "fmt", "test", "wasm-bridges"],
            # Routed by `trunk build --config docs/Trunk.toml`, whose HTML target links its manifest.
            "rust.obc-web-demo": ["clippy", "fmt", "test", "wasm"],
            "swift.obckit-host": ["ios-unit"],
            "ci.docs": ["docs"],
            "web.builder-vitest": ["web"],
            "python.repository-tools": ["guards", "selection"],
            "python.firmware-tools": ["test"],
            "python.builder": ["builder-python"],
            "ci.ui-snapshots": ["ui-snapshots"],
            # The sweep job compiles the simulator, so it is a route for the simulator's own
            # suite as well: a job that builds a package runs whenever that package is selected.
            "rust.obc-sim": ["clippy", "fmt", "test", "ui-snapshots"],
            "web.demo-browser": ["wasm"],
            "web.builder-browser": ["web-browser"],
        }
        for suite_id, jobs in expected.items():
            with self.subTest(suite=suite_id):
                self.assertEqual(self.routes[suite_id], jobs)

    def test_every_gated_job_gates_on_its_own_name_and_reports_to_the_gate(self) -> None:
        gate = registry.aggregate_job(self.root)
        for name, job in self.jobs.items():
            if name == gate:
                continue
            with self.subTest(job=name):
                self.assertIn(name, self.jobs[gate].needs)
                if job.plan_gated:
                    self.assertEqual(job.gates_on, name)

    def test_selected_job_set_per_change_class(self) -> None:
        """The exact conditional job set per class. The last three classes are the crates whose
        only build is a non-Cargo command, which a subset assertion let regress once."""

        cases = [
            ("documentation only", ["docs/content/ride.md"], ["docs"]),
            ("leaf Rust crate", ["host/obc-bench/src/main.rs"], ["clippy", "fmt", "test"]),
            (
                "foundational Rust crate",
                ["firmware/obc-crc/src/lib.rs"],
                ["boot", "builder-python", "clippy", "desktop", "desktop-frontend", "device", "embedded", "fmt", "test", "ui-snapshots", "wasm", "wasm-bridges"],
            ),
            (
                "shared vectors",
                ["specs/vectors/obcm-v2.json"],
                ["clippy", "device", "fmt", "ios-unit", "test", "wasm-bridges", "web"],
            ),
            (
                "desktop launch harness",
                ["apps/obc-desktop/e2e/launch.py"],
                ["desktop", "desktop-frontend", "desktop-launch", "fmt", "wasm-bridges"],
            ),
            ("iOS application", ["companion-ios/OBCCompanion/App.swift"], ["ios-app"]),
            (
                "web only",
                ["builder/app/src/lib/panel.ts"],
                ["desktop", "desktop-frontend", "desktop-launch", "fmt", "wasm-bridges", "web", "web-browser"],
            ),
            (
                "workflow",
                [".github/workflows/ci.yml"],
                ["boot", "builder-python", "clippy", "deny", "desktop", "desktop-frontend", "desktop-launch", "device", "docs", "embedded", "fmt", "ios-app", "ios-unit", "test", "ui-snapshots", "wasm", "wasm-bridges", "web", "web-browser"],
            ),
            (
                "nextest configuration",
                [".config/nextest.toml"],
                ["boot", "builder-python", "clippy", "deny", "desktop", "desktop-frontend", "desktop-launch", "device", "docs", "embedded", "fmt", "ios-app", "ios-unit", "test", "ui-snapshots", "wasm", "wasm-bridges", "web", "web-browser"],
            ),
            # The web demo is built only by `trunk build`, the OBCKit package is compiled into the
            # app only by `xcodebuild`, and tools/fixtures.py is run only by a workflow step.
            ("web demo crate", ["apps/obc-web-demo/src/lib.rs"], ["clippy", "fmt", "test", "wasm"]),
            ("web demo Trunk target", ["docs/index.html"], ["docs", "wasm", "wasm-bridges"]),
            ("web demo browser harness", ["apps/obc-web-demo/tests/browser/ride-log.test.js"], ["clippy", "fmt", "test", "wasm"]),
            (
                "OBCKit package source",
                ["companion-ios/Packages/OBCKit/Sources/OBCTransport/BLE/Client.swift"],
                ["ios-app", "ios-unit"],
            ),
            (
                "repository tooling",
                ["tools/fixtures.py"],
                ["desktop", "desktop-frontend", "test", "wasm-bridges"],
            ),
        ]
        for name, paths, expected in cases:
            with self.subTest(change=name):
                jobs = self.jobs_for(*paths) - self.unconditional
                self.assertEqual(sorted(jobs), expected)

    def test_snapshot_sweep_requires_its_own_rendering_inputs(self):
        for path, selected in [("testing/coverage-policy.toml", False), (".github/workflows/ci.yml", False),
                               ("firmware/obc-render/src/stroke.rs", True), ("firmware/ui-snapshots.sha256", True),
                               ("firmware/obc-app/i18n/en.toml", True), ("firmware/obc-app/assets/landmark.bin", True),
                               ("firmware/obc-render/fonts/terminus/font.bdf", True)]:
            plan = registry.select_suites(self.inventory, [path], self.graph, self.routes,
                                          unconditional=self.unconditional)
            actual = next(item.selected for item in plan.suites if item.suite["id"] == "ci.ui-snapshots")
            self.assertEqual(actual, selected, path)

    def test_shipped_fast_gate_never_claims_fixture_live_or_manual_suites(self):
        claims = registry.gate_claims(self.inventory.suites, self.graph)["test"]
        by_id = {suite["id"]: suite for suite in self.inventory.suites}
        self.assertIn("rust.obc-route", claims)
        self.assertFalse({id for id in claims if by_id[id]["level"] in {"fixture", "live", "hardware"}
                          or by_id[id]["scheduled"] == "manual"})

class ExceptionIssueStateTests(unittest.TestCase):
    def suite(self, reference: str, field: str = "quarantine", name: str = "demo") -> dict:
        return {"id": name, field: {"reason": "Pending repair", "issue": reference}}

    def response(self, value: object) -> subprocess.CompletedProcess:
        return subprocess.CompletedProcess([], 0, stdout=json.dumps(value))

    @patch.object(registry.subprocess, "run")
    def test_local_and_full_references_share_one_lookup(self, run) -> None:
        run.return_value = self.response({"state": "open"})
        suites = [self.suite("#7"), self.suite("https://github.com/OWNER/REPO/issues/007", "sleep_exception")]
        self.assertEqual(registry.check_issue_states(suites, "owner/repo"), 1)
        run.assert_called_once_with(
            ["gh", "api", "repos/owner/repo/issues/7"],
            check=True, capture_output=True, text=True, timeout=20,
        )

    @patch.object(registry.subprocess, "run")
    def test_distinct_issues_report_all_owning_fields(self, run) -> None:
        run.side_effect = [self.response({"state": "closed"}), self.response({"state": "closed"})]
        suites = [self.suite("#1"), self.suite("#1", "cadence_conflict", "other"), self.suite("#2", "budget_exception")]
        with self.assertRaises(registry.RegistryError) as caught:
            registry.check_issue_states(suites, "owner/repo")
        for owner in ("demo.quarantine", "other.cadence_conflict", "demo.budget_exception"):
            self.assertIn(owner, str(caught.exception))
        self.assertEqual(run.call_count, 2)

    @patch.object(registry.subprocess, "run")
    def test_invalid_references_are_rejected_before_any_request(self, run) -> None:
        for invalid in ("garbage", "https://example.com/o/r/issues/3", "https://github.com/o/r?bad/issues/3"):
            with self.subTest(reference=invalid), self.assertRaises(registry.RegistryError):
                registry.check_issue_states([self.suite("#1"), self.suite(invalid)], "owner/repo")
        run.assert_not_called()

    @patch.object(registry.subprocess, "run")
    def test_missing_reason_and_repository_are_rejected_before_requests(self, run) -> None:
        suite = self.suite("#1")
        suite["quarantine"]["reason"] = ""
        with self.assertRaises(registry.RegistryError):
            registry.check_issue_states([suite], "owner/repo")
        with self.assertRaises(registry.RegistryError):
            registry.check_issue_states([self.suite("#1")], "not-a-repository")
        run.assert_not_called()

    @patch.object(registry.subprocess, "run")
    def test_pull_requests_and_malformed_responses_are_not_open_issues(self, run) -> None:
        for response in ({"state": "open", "pull_request": {}}, {}, [], {"state": []}, {"state": "unknown"}):
            with self.subTest(response=response):
                run.return_value = self.response(response)
                with self.assertRaisesRegex(registry.RegistryError, "demo.quarantine: owner/repo#1"):
                    registry.check_issue_states([self.suite("#1")], "owner/repo")

    @patch.object(registry.subprocess, "run")
    def test_api_auth_timeout_and_decode_errors_are_visible_without_retry(self, run) -> None:
        failures = [
            subprocess.CalledProcessError(1, ["gh"], stderr="HTTP 403: authentication failed"),
            subprocess.TimeoutExpired(["gh"], 20),
            FileNotFoundError("gh is not installed"),
        ]
        for failure in failures:
            with self.subTest(failure=type(failure).__name__):
                run.reset_mock(side_effect=True)
                run.side_effect = failure
                with self.assertRaisesRegex(registry.RegistryError, "demo.quarantine: owner/repo#1"):
                    registry.check_issue_states([self.suite("#1")], "owner/repo")
                run.assert_called_once()
        run.reset_mock(side_effect=True)
        run.return_value = subprocess.CompletedProcess([], 0, stdout="not JSON")
        with self.assertRaisesRegex(registry.RegistryError, "could not check issue"):
            registry.check_issue_states([self.suite("#1")], "owner/repo")
        run.assert_called_once()

if __name__ == "__main__":
    unittest.main()


class CargoCadenceTests(unittest.TestCase):
    def test_whole_target_owners_partition_fast_fixture_and_manual_work(self):
        root = Path('.')
        units = [registry.Discovered('rust-target', f'demo:{target}', '', f'Cargo.toml:{kind}')
                 for target, kind in [('demo', 'lib'), ('captured', 'test'), ('network', 'test'), ('writer', 'example')]]
        fast = {'kind': 'rust-package', 'name': 'demo', 'exclude_targets': ['captured', 'network', 'writer']}
        self.assertEqual([unit.name for unit in units if registry.ownership_matches(root, fast, unit)], ['demo:demo'])
        suites, matches = [], {}
        for name, level, cadence, target in [('fast', 'unit', 'affected', 'demo'), ('fixture', 'fixture', 'affected', 'captured'),
                                              ('live', 'live', 'never', 'network'), ('writer', 'contract', 'never', 'writer')]:
            suites.append({'id': name, 'level': level, 'pull_request': cadence})
            owner = {'kind': 'rust-package', 'name': 'demo', 'targets': [target]}
            matches[name] = [unit for unit in units if registry.ownership_matches(root, owner, unit)]
        inventory = registry.Inventory(suites, [], units, matches)
        self.assertEqual(registry.cargo_filter(inventory, 'fast'), '(package(=demo) & binary(=demo))')
        self.assertEqual(registry.cargo_filter(inventory, 'fixtures'), '(package(=demo) & binary(=captured))')
        with self.assertRaisesRegex(registry.RegistryError, 'no Rust binaries'):
            registry.cargo_filter(registry.Inventory([], [], [], {}), 'fixtures')

    def test_step_condition_does_not_replace_job_selection_gate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workflow = root / '.github/workflows/ci.yml'
            workflow.parent.mkdir(parents=True)
            workflow.write_text("jobs:\n  test:\n    runs-on: ubuntu-latest\n    needs: selection\n"
                                "    if: contains(fromJSON(needs.selection.outputs.jobs), 'test')\n"
                                "    steps:\n      - name: optional suite\n"
                                "        if: contains(fromJSON(needs.selection.outputs.plan).selected_suite_ids, 'ci.ui-snapshots')\n"
                                "        run: echo run\n")
            self.assertEqual(registry.workflow_jobs(root)['test'].gates_on, 'test')

    def test_level_selection_never_runs_manual_fixture_writers(self):
        suites = [{'id': name, 'level': 'contract', 'surface': 'support', 'scheduled': cadence,
                   'pull_request': 'never' if cadence == 'manual' else 'affected', 'command': 'false'}
                  for name, cadence in [('required', 'none'), ('writer', 'manual')]]
        inventory = registry.Inventory(suites, [], [], {})
        plan = registry.select_by_level(inventory, {'required': ['test']}, 'contract', 'support')
        self.assertEqual([item.suite['id'] for item in plan.selected], ['required'])

    def test_xcuitest_file_owners_do_not_overlap(self):
        unit = registry.Discovered('xcuitest', 'WebsiteScreenshotTests', 'ios/WebsiteScreenshotTests.swift')
        owner = {'kind': 'path', 'source': 'xcuitest', 'pattern': 'ios/*.swift', 'exclude': [unit.path]}
        self.assertFalse(registry.ownership_matches(Path('.'), owner, unit))
        self.assertTrue(registry.ownership_matches(Path('.'), {**owner, 'exclude': []}, unit))
