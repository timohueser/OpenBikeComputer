from __future__ import annotations

import copy
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

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
            "tools/tests/test_tool.py": "def test_x(): pass",
            "firmware/tools/tests/test_firmware.py": "def test_x(): pass",
            "ops/weather/tests/test_weather.py": "def test_x(): pass",
            "builder/tests/test_builder.py": "def test_x(): pass",
            "tools/rain-radar-demo/tests/radar.test.ts": "test('x', () => {})",
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
                "python-test",
                "rain-radar-test",
                "xcuitest",
                "swift-target",
                "workflow-command",
            },
        )

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
        case("budget issue", lambda s, c, d: s["suite"][0].update(budget_exception={"reason": "slow"}), "open GitHub issue")
        case("quarantine issue", lambda s, c, d: s["suite"][0].update(quarantine={"reason": "flake"}), "open GitHub issue")
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

    def test_real_sleep_requires_an_open_issue_exception(self) -> None:
        source = self.root / "tools/tests/test_wait.py"
        source.parent.mkdir(parents=True)
        source.write_text("import time\ntime.sleep(1)\n", encoding="utf-8")
        suites, coverage, _ = self.base_documents()
        suites["suite"][0].update(
            surface="python",
            command="python3 -m unittest tools.tests.test_wait",
            ownership=[{"kind": "path", "source": "python-test", "pattern": "tools/tests/test_*.py"}],
        )
        discovered = [registry.Discovered("python-test", "tools/tests/test_wait.py", "tools/tests/test_wait.py")]
        with self.assertRaisesRegex(registry.RegistryError, "real sleep"):
            registry.validate(self.root, suites, coverage, discovered)
        suites["suite"][0]["sleep_exception"] = {"reason": "bounded watchdog", "issue": "#1236"}
        registry.validate(self.root, suites, coverage, discovered)

    def test_workflow_parser_handles_inline_blocks_and_matrix_commands(self) -> None:
        workflow = self.root / ".github/workflows/ci.yml"
        workflow.parent.mkdir(parents=True)
        workflow.write_text(
            "steps:\n"
            "  - run: npm test\n"
            "  - run: |\n"
            "      python3 tools/check.py\n"
            "      echo ignored\n"
            '  - { name: test, cmd: "cargo test --locked" }\n',
            encoding="utf-8",
        )
        commands = {item.name for item in registry.discover_workflow(self.root)}
        self.assertEqual(commands, {"npm test", "python3 tools/check.py", "cargo test --locked"})


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
            self._suite("python.weather", "path", "python-test", pattern="weather/tests/test_*.py", triggers=["weather/**"]),
            self._suite("python.tool", "path", "python-test", pattern="tools/tests/test_*.py", triggers=["tools/tool.py"]),
            self._suite("python.builder", "path", "python-test", pattern="builder/tests/test_*.py", triggers=["builder/server/**/*.py"]),
            self._suite("fixture.consumer", "path", "python-test", pattern="fixtures/tests/test_*.py", fixtures=["catalog"]),
            self._suite("ci.policy", "workflow", "python3 tools/check.py", triggers=["testing/**", ".github/workflows/**"]),
            self._suite("ci.docs", "workflow", "python3 docs/build.py", triggers=["docs/**"]),
            self._suite("missing.route", "path", "rain-radar-test", pattern="demo/tests/*.test.ts", triggers=["demo/**"]),
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
        self.routes = {suite["id"]: ["rust"] for suite in self.suites}
        self.routes.update(
            {
                "rust.desktop": ["desktop"],
                "web.unit": ["web"],
                "swift.contract": ["ios"],
                "python.weather": ["rust"],
                "python.tool": ["always"],
                "python.builder": ["rust"],
                "fixture.consumer": ["always"],
                "ci.policy": ["always"],
                "ci.docs": ["docs"],
                "missing.route": [],
            }
        )

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
        return registry.select_suites(self.inventory, [path], self.graph, self.routes)

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
            "weather/probe.py": {"python.weather"},
            "tools/tool.py": {"python.tool"},
            "builder/server/nested/routes.py": {"python.builder"},
            "fixtures/catalog.toml": {"fixture.consumer"},
            ".github/workflows/release.yml": {"ci.policy"},
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

    def test_json_reports_platforms_and_non_selected_reasons(self) -> None:
        data = registry.selection_plan_data(self.plan("apps/desktop/src/main.rs"))
        desktop = next(item for item in data["suites"] if item["id"] == "rust.desktop")
        web = next(item for item in data["suites"] if item["id"] == "web.unit")
        self.assertEqual(desktop["platforms"], ["linux", "macos", "windows"])
        self.assertEqual(web["reasons"], ["no changed path, Cargo edge, or required cadence selected this suite"])

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


if __name__ == "__main__":
    unittest.main()
